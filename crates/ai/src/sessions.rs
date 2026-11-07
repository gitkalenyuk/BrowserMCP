//! The life of a session after its start: continuing a stopped chat, stopping, closing,
//! restoring saved sessions at startup, and answering approval requests.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bmcp_core::{Caller, CmdError, Ctx, ErrorCode};
use serde_json::json;

use crate::cli::access::AccessSettings;
use crate::cli::permission::{Decision, PermissionRequest};
use crate::session::{Session, SessionInfo, Status, saved_sessions, topics};
use crate::{AiService, lock};

/// How long a finished backend may take to clean up before a restart or close gives up on it.
const TASK_GRACE: Duration = Duration::from_secs(15);

/// `claude-code-12` → 12.
fn id_number(id: &str) -> Option<u64> {
    id.rsplit('-').next()?.parse().ok()
}

impl AiService {
    /// List the sessions saved by an earlier run (stopped), and number new sessions after
    /// every transcript on disk, so a new session never appends to an old one's file.
    pub(crate) fn restore_sessions(&self) {
        let dir = self.sessions_dir();
        let highest = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                id_number(name.split('.').next()?)
            })
            .max()
            .unwrap_or(0);
        self.counter.fetch_max(highest, Ordering::Relaxed);
        let mut sessions = lock(&self.sessions);
        for (saved, log) in saved_sessions(&dir) {
            let id = saved.id.clone();
            sessions.insert(id, Arc::new(Session::restore(saved, log)));
        }
    }

    /// Send the next message. A stopped or failed session is continued first (same chat,
    /// same conversation): it never turns into a new session.
    pub async fn send(&self, ctx: &Ctx, id: &str, text: String) -> Result<SessionInfo, CmdError> {
        let session = self.session(id)?;
        // Continuing is serialized: a second send that waited here finds the session already
        // continued and only queues its message. The guard is held until the message is queued,
        // so a close cannot slip in between.
        let _turn = match session.needs_restart() {
            true => Some(session.lifecycle().await),
            false => None,
        };
        if session.is_closed() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("AI session '{id}' was closed"),
            )
            .with_hint("Start a new session with ai_session_start."));
        }
        if session.needs_restart() {
            self.continue_session(ctx, &session).await?;
        }
        session.send(text).map_err(|m| {
            CmdError::invalid_params(m)
                .with_hint("Try again, or start a new session with ai_session_start.")
        })?;
        Ok(session.info())
    }

    /// Start a new backend for an ended session, after the previous one has finished. The
    /// caller holds [`Session::lifecycle`], so only one call at a time gets here per session:
    /// the old backend is over before the new one starts, and its last status / approval
    /// updates cannot land on the new run.
    async fn continue_session(&self, ctx: &Ctx, session: &Arc<Session>) -> Result<(), CmdError> {
        // Make sure the old run is over (it may still be reporting `stopped`).
        session.cancel();
        if let Some(mut task) = session.take_task()
            && tokio::time::timeout(TASK_GRACE, &mut task).await.is_err()
        {
            task.abort();
        }
        let input = session.restart(ctx.events.clone());
        // A session restored after a browser restart has no tab group yet.
        let name = self
            .resolve(&session.provider)
            .map(|r| r.preset.name)
            .unwrap_or("AI");
        let n = id_number(&session.id).unwrap_or(0);
        let _ = self.agent_color(ctx, &session.id, n, name);
        if let Err(err) = self.spawn_backend(ctx, session, input) {
            session.cancel();
            session.set_status(Status::Failed);
            return Err(err);
        }
        Ok(())
    }

    /// Stop a session's backend (the session stays and can be continued).
    pub async fn stop(&self, id: &str) -> Result<SessionInfo, CmdError> {
        let session = self.session(id)?;
        let generation = session.generation();
        session.cancel();
        session.cancel_permissions();
        // Give the task a moment to report `stopped` (returns as soon as it has; a CLI agent
        // may need a few seconds to be killed on a busy machine), or until a message sent
        // meanwhile has continued the chat with a new run.
        for _ in 0..250 {
            if session.status().ended() || session.generation() != generation {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(session.info())
    }

    /// Stop (if running) and remove a session: it is no longer listed, now or after a
    /// restart. The transcript file stays unless `delete_transcript`.
    pub async fn close(
        &self,
        ctx: &Ctx,
        id: &str,
        delete_transcript: bool,
    ) -> Result<(), CmdError> {
        let session = self.session(id)?;
        // Not while a send is continuing it (that send would start a backend for a closed chat).
        let _turn = session.lifecycle().await;
        session.mark_closed();
        session.cancel();
        session.cancel_permissions();
        if let Some(mut task) = session.take_task()
            && tokio::time::timeout(TASK_GRACE, &mut task).await.is_err()
        {
            task.abort();
        }
        lock(&self.sessions).remove(id);
        session.remove_files(delete_transcript);
        // Normally removed by the Claude Code task itself; not when it had to be aborted.
        let _ = std::fs::remove_file(self.sessions_dir().join(format!("{id}.mcp.json")));
        ctx.events
            .emit(topics::SESSION_CLOSED, json!({ "session": id }));
        Ok(())
    }

    /// Approval requests waiting for an answer, of one session or all.
    pub fn permissions(&self, session: Option<&str>) -> Result<Vec<PermissionRequest>, CmdError> {
        let sessions: Vec<Arc<Session>> = match session {
            Some(id) => vec![self.session(id)?],
            None => lock(&self.sessions).values().cloned().collect(),
        };
        let mut list: Vec<PermissionRequest> = sessions
            .iter()
            .flat_map(|s| s.pending_permissions())
            .collect();
        list.sort_by_key(|r| r.created_ms);
        Ok(list)
    }

    /// One of this browser's own AI chats: an API agent (`Caller::Agent`), or a CLI agent
    /// calling the browser's MCP server with `?agent=<its session id>`. Other MCP clients
    /// (whose agent name is not a session) are not: they are clients the user connected.
    pub fn is_own_agent(&self, caller: &Caller) -> bool {
        match caller {
            Caller::Agent { .. } => true,
            _ => caller
                .agent_id()
                .is_some_and(|id| lock(&self.sessions).contains_key(id)),
        }
    }

    /// Approvals and access settings are the user's call: refused for the browser's own agents,
    /// which could otherwise approve each other's requests or raise their own access.
    pub fn refuse_own_agent(&self, caller: &Caller, what: &str) -> Result<(), CmdError> {
        if !self.is_own_agent(caller) {
            return Ok(());
        }
        Err(CmdError::new(
            ErrorCode::NeedsApproval,
            format!("an AI agent cannot {what}"),
        )
        .with_hint("Ask the user to do it in the browser's AI panel (ai_handoff)."))
    }

    /// An agent may start a chat of its own with the provider's defaults or less, never with
    /// more access (see [`AccessSettings::widened_by`]).
    pub(crate) fn refuse_agent_widening(
        &self,
        caller: &Caller,
        defaults: &AccessSettings,
        over: &AccessSettings,
    ) -> Result<(), CmdError> {
        match defaults.widened_by(over) {
            Some(field) if self.is_own_agent(caller) => Err(CmdError::new(
                ErrorCode::NeedsApproval,
                format!(
                    "an AI agent cannot start a chat with more access than the defaults ({field})"
                ),
            )
            .with_hint(
                "Omit permission_mode / sandbox / approval_policy / cwd / add_dirs, or ask the \
                 user to start that chat in the AI panel.",
            )),
            _ => Ok(()),
        }
    }

    /// Answer approval request `id`. Only people and the clients they connected may: never
    /// one of the browser's own agents (not even for another chat).
    pub fn answer_permission(
        &self,
        caller: &Caller,
        id: &str,
        decision: Decision,
    ) -> Result<Vec<PermissionRequest>, CmdError> {
        self.refuse_own_agent(caller, "answer approval requests")?;
        let session = lock(&self.sessions)
            .values()
            .find(|s| s.has_permission(id))
            .cloned()
            .ok_or_else(|| {
                CmdError::new(
                    ErrorCode::NotFound,
                    format!("no pending approval request '{id}'"),
                )
                .with_hint(
                    "It may have been answered or cancelled already; call ai_permissions_list.",
                )
            })?;
        // An MCP client named like the chat that asks (not one of ours once it is closed).
        if caller.agent_id() == Some(session.id.as_str()) {
            return Err(CmdError::new(
                ErrorCode::NeedsApproval,
                "an agent cannot approve its own requests",
            )
            .with_hint("A person (or another client) must answer it."));
        }
        session.answer_permission(id, decision).ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotFound,
                format!("approval request '{id}' was already answered"),
            )
        })
    }

    /// The saved access defaults of a provider.
    pub fn access_settings(&self, provider: &str) -> Result<AccessSettings, CmdError> {
        self.resolve(provider)?; // unknown id → NOT_FOUND
        Ok(lock(&self.config).settings(provider).access)
    }

    /// Change and save a provider's access defaults; folders must exist.
    pub fn set_access_settings(
        &self,
        provider: &str,
        f: impl FnOnce(&mut AccessSettings),
    ) -> Result<AccessSettings, CmdError> {
        let mut access = self.access_settings(provider)?;
        f(&mut access);
        access.resolve(&self.workspace_dir()).map_err(|e| {
            CmdError::invalid_params(e)
                .with_hint("Use an existing folder, or \"\" for the default.")
        })?;
        let saved = access.clone();
        self.update_config(|config| {
            config
                .providers
                .entry(provider.to_string())
                .or_default()
                .access = saved;
        })?;
        Ok(access)
    }

    /// The folder CLI agents work in when none is set.
    pub fn default_cwd(&self) -> std::path::PathBuf {
        self.workspace_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_numbers() {
        assert_eq!(id_number("claude-code-12"), Some(12));
        assert_eq!(id_number("custom-1"), Some(1));
        assert_eq!(id_number("codex"), None);
    }
}
