//! The life of a session after its start: continuing a stopped chat, stopping, interrupting,
//! changing the model, editing a message, closing (archiving), reopening, renaming, deleting,
//! restoring saved sessions at startup, and answering approval requests.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bmcp_core::{Caller, CmdError, Ctx, ErrorCode};
use serde_json::json;

use crate::cli::access::AccessSettings;
use crate::cli::permission::{Decision, PermissionRequest};
use crate::config::Kind;
use crate::session::{
    AgentEvent, Session, SessionInfo, Status, history_from_transcript, rewind_history,
    saved_sessions, topics,
};
use crate::{AiService, lock};

/// How long a finished backend may take to clean up before a restart or close gives up on it.
const TASK_GRACE: Duration = Duration::from_secs(15);

/// The chat so far as text, for an agent that starts a new conversation after an edit.
fn earlier_context(earlier: &[(String, String)]) -> String {
    let mut text = String::from(
        "(Earlier in this chat — the conversation could not be rewound, so here it is for \
         context:)\n",
    );
    for (role, line) in earlier {
        let who = if role == "user" { "User" } else { "Assistant" };
        text.push_str(&format!("\n{who}: {line}\n"));
    }
    text
}

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
        // Provider names (the title of a chat without messages), resolved once per provider.
        let mut names: std::collections::HashMap<String, String> = Default::default();
        let mut restored = Vec::new();
        for (saved, log) in saved_sessions(&dir) {
            let id = saved.id.clone();
            // Written before titles existed: saved again with the new fields.
            let migrate = saved.updated_ms == 0;
            let name = names
                .entry(saved.provider.clone())
                .or_insert_with(|| {
                    self.resolve(&saved.provider)
                        .map(|r| r.name)
                        .unwrap_or_else(|_| saved.provider.clone())
                })
                .clone();
            let mut session = Session::restore(saved, log);
            session.provider_name = name;
            if migrate {
                session.save_meta();
            }
            restored.push((id, Arc::new(session)));
        }
        lock(&self.sessions).extend(restored);
    }

    /// Send the next message. A stopped or failed session is continued first (same chat,
    /// same conversation): it never turns into a new session. A busy one takes the message
    /// while it works (queued until the agent has it).
    ///
    /// Messages still queued when a run ended by itself (the CLI failed or exited before it
    /// took them; a stop, close or edit drops them instead) stay queued while the chat is
    /// stopped. The message that continues the chat takes them along: the new run gets their
    /// texts, then this one, as its first input (like an edit sends the messages it rewound
    /// with `Rewind::prefix`), and each of them is announced as taken (`ai.message_taken`), so
    /// the chat shows truthfully that the agent has them.
    pub async fn send(&self, ctx: &Ctx, id: &str, text: String) -> Result<SessionInfo, CmdError> {
        let session = self.session(id)?;
        // Continuing is serialized: a second send that waited here finds the session already
        // continued and only queues its message. The guard is held until the message is queued,
        // so a close cannot slip in between.
        let _turn = match session.needs_restart() {
            true => Some(session.lifecycle().await),
            false => None,
        };
        self.ensure_open(&session)?;
        let mut carried = Vec::new();
        if session.needs_restart() {
            self.continue_session(ctx, &session).await?;
            carried = session.queued_messages();
        }
        let refused = |m: String| {
            CmdError::invalid_params(m)
                .with_hint("Try again, or start a new session with ai_session_start.")
        };
        if carried.is_empty() {
            session.send(text).map_err(refused)?;
        } else {
            let mut parts: Vec<String> = carried.iter().map(|(_, t)| t.clone()).collect();
            parts.push(text.clone());
            session.send_as(text, parts.join("\n\n")).map_err(refused)?;
            let indexes: Vec<usize> = carried.iter().map(|(i, _)| *i).collect();
            session.take(&indexes);
        }
        Ok(session.info())
    }

    /// Run lifecycle work (close, delete, edit) in a task of its own and wait for it. The
    /// caller may be the chat's own agent: an API agent calls the command from inside its
    /// turn, which the work cancels (and then waits for), and a CLI agent's MCP request ends
    /// with the process the work kills. Run inline, the work would be dropped halfway with
    /// that turn or request; as a task of its own it always finishes.
    pub(crate) async fn detached<T: Send + 'static>(
        work: impl std::future::Future<Output = Result<T, CmdError>> + Send + 'static,
    ) -> Result<T, CmdError> {
        tokio::spawn(work)
            .await
            .unwrap_or_else(|e| Err(CmdError::internal(format!("AI session task failed: {e}"))))
    }

    /// Deleted (gone) or archived (closed: reopen it first) chats take no messages.
    fn ensure_open(&self, session: &Session) -> Result<(), CmdError> {
        if session.is_closed() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("AI session '{}' was deleted", session.id),
            )
            .with_hint("Start a new session with ai_session_start."));
        }
        if session.is_archived() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("AI session '{}' was closed", session.id),
            )
            .with_hint(
                "Reopen it with ai_session_reopen, or start a new session with ai_session_start.",
            ));
        }
        Ok(())
    }

    /// Wait for the backend task of the last run to finish (after a cancel).
    async fn finish_task(session: &Session) {
        if let Some(mut task) = session.take_task()
            && tokio::time::timeout(TASK_GRACE, &mut task).await.is_err()
        {
            task.abort();
        }
    }

    /// Start a new backend for an ended session, after the previous one has finished. The
    /// caller holds [`Session::lifecycle`], so only one call at a time gets here per session:
    /// the old backend is over before the new one starts, and its last status / approval
    /// updates cannot land on the new run.
    async fn continue_session(&self, ctx: &Ctx, session: &Arc<Session>) -> Result<(), CmdError> {
        // Make sure the old run is over (it may still be reporting `stopped`).
        session.cancel();
        Self::finish_task(session).await;
        let input = session.restart(ctx.events.clone());
        // A session restored after a browser restart has no tab group yet.
        let name = self
            .resolve(&session.provider)
            .map(|r| r.name)
            .unwrap_or_else(|_| "AI".to_string());
        let n = id_number(&session.id).unwrap_or(0);
        let _ = self.agent_color(ctx, &session.id, n, &name);
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
        session.drop_queued();
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

    /// The session, publishing on the live bus (see [`Session::attach_events`]).
    fn live(&self, ctx: &Ctx, id: &str) -> Result<Arc<Session>, CmdError> {
        let session = self.session(id)?;
        session.attach_events(&ctx.events);
        Ok(session)
    }

    /// Stop the current turn but keep the chat alive (Claude Code / Codex keep their process,
    /// API agents their loop). Waits a moment for the turn to end.
    pub async fn interrupt(&self, ctx: &Ctx, id: &str) -> Result<SessionInfo, CmdError> {
        let session = self.live(ctx, id)?;
        if session.interrupt() {
            for _ in 0..250 {
                if session.status() != Status::Running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        Ok(session.info())
    }

    /// Use `model` in this chat from the next turn on ("" / "default": the provider's).
    pub fn set_model(&self, ctx: &Ctx, id: &str, model: &str) -> Result<SessionInfo, CmdError> {
        let session = self.live(ctx, id)?;
        let model = Some(model.trim().to_string()).filter(|m| !crate::models::is_default(m));
        session.set_model(model.clone());
        // A running CLI switches at once (its next turn); without a model of its own the chat
        // goes back to the provider's default.
        let effective = model.or_else(|| self.resolve(&session.provider).ok()?.model);
        session.send_model(effective);
        Ok(session.info())
    }

    /// Stop (if running) and archive a session: it leaves the open chats (`ai.session_closed`)
    /// and is listed with `ai.sessions_list {archived: true}`; every file stays. With
    /// `delete_transcript` (the older form of this command) it is deleted instead.
    pub async fn close(
        &self,
        ctx: &Ctx,
        id: &str,
        delete_transcript: bool,
    ) -> Result<(), CmdError> {
        if delete_transcript {
            return self.delete(ctx, id).await;
        }
        let session = self.live(ctx, id)?;
        if session.is_archived() {
            return Ok(());
        }
        // Not while a send is continuing it (that send would start a backend for a closed chat).
        let _turn = session.lifecycle().await;
        // A delete held the guard meanwhile (it must not be written back), or another close
        // archived it already.
        if session.is_closed() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("AI session '{id}' was deleted"),
            ));
        }
        if session.is_archived() {
            return Ok(());
        }
        self.end_run(&session).await;
        session.set_archived(true);
        ctx.events
            .emit(topics::SESSION_CLOSED, json!({ "session": id }));
        Ok(())
    }

    /// Stop whatever runs in `session` and wait for it (caller holds the lifecycle guard).
    async fn end_run(&self, session: &Session) {
        session.cancel();
        session.cancel_permissions();
        Self::finish_task(session).await;
        session.drop_queued();
        if !session.status().ended() {
            session.set_status(Status::Stopped);
        }
        // Normally removed by the Claude Code task itself; not when it had to be aborted.
        let _ = std::fs::remove_file(self.sessions_dir().join(format!("{}.mcp.json", session.id)));
    }

    /// Bring an archived chat back to the open chats; it continues like a stopped chat.
    pub async fn reopen(&self, ctx: &Ctx, id: &str) -> Result<SessionInfo, CmdError> {
        let session = self.live(ctx, id)?;
        let _turn = session.lifecycle().await;
        if session.is_closed() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("AI session '{id}' was deleted"),
            ));
        }
        if session.is_archived() {
            session.set_archived(false);
        }
        Ok(session.info())
    }

    /// Stop (if running) and remove a session for good, with every file of it.
    pub async fn delete(&self, ctx: &Ctx, id: &str) -> Result<(), CmdError> {
        let session = self.live(ctx, id)?;
        let _turn = session.lifecycle().await;
        session.mark_closed();
        self.end_run(&session).await;
        lock(&self.sessions).remove(id);
        session.remove_files();
        ctx.events
            .emit(topics::SESSION_CLOSED, json!({ "session": id }));
        ctx.events
            .emit(topics::SESSION_DELETED, json!({ "session": id }));
        Ok(())
    }

    /// Rename a chat ("" = back to the automatic title).
    pub fn rename(&self, ctx: &Ctx, id: &str, title: &str) -> Result<SessionInfo, CmdError> {
        let session = self.live(ctx, id)?;
        session.rename(Some(title.to_string()));
        Ok(session.info())
    }

    /// Edit user message `index`: rewind the conversation to just before it and send `text`
    /// instead (the transcript is cut there; the new text follows as a user message).
    pub async fn edit(
        &self,
        ctx: &Ctx,
        id: &str,
        index: usize,
        text: String,
    ) -> Result<SessionInfo, CmdError> {
        let session = self.live(ctx, id)?;
        let resolved = self.resolve(&session.provider)?;
        let _turn = session.lifecycle().await;
        self.ensure_open(&session)?;
        let count = session.user_count();
        if index >= count {
            return Err(CmdError::invalid_params(format!(
                "chat {id} has {count} user message(s); there is no message {index}"
            ))
            .with_hint("index is the 0-based number of the user message in the chat."));
        }
        // The agent may be in the middle of it: end the run first.
        self.end_run(&session).await;
        let rewind = session.rewind(index).ok_or_else(|| {
            CmdError::invalid_params(format!("no user message {index} in chat {id}"))
        })?;
        // What the agent gets: the messages it lost with the rewind, then the new text.
        let mut parts = rewind.prefix.clone();
        let mut lost = false;
        match resolved.kind {
            Kind::Api(_) => {
                let history = session.api_history();
                let kept = rewind_history(&history, &rewind.user_texts, index)
                    .unwrap_or_else(|| history_from_transcript(&session.transcript()));
                session.set_api_history(kept);
                // The kept history holds the earlier messages itself.
                parts.clear();
            }
            Kind::ClaudeCode => match &rewind.resume_at {
                Some(at) if session.conversation().is_some() => {
                    session.prepare_resume(Some(at.clone()), None);
                }
                _ => {
                    // No point to resume at (a chat from before checkpoints were kept, or
                    // nothing answered yet): a new conversation, given the chat so far.
                    session.forget_conversation();
                    session.prepare_resume(None, None);
                    if rewind.turns_before {
                        parts = vec![earlier_context(&rewind.earlier)];
                        lost = true;
                    }
                }
            },
            Kind::Codex => {
                if session.conversation().is_some() {
                    session.prepare_resume(None, Some(rewind.turns_after));
                } else {
                    session.prepare_resume(None, None);
                }
            }
        }
        // The cut first: a window that applies it drops everything from `index` on, so the
        // notice must come after it.
        ctx.events.emit(
            topics::SESSION_REWOUND,
            json!({ "session": id, "index": index }),
        );
        if lost {
            session.apply(AgentEvent::resume_lost(&resolved.name));
        }
        self.continue_session(ctx, &session).await?;
        parts.push(text.clone());
        session
            .send_as(text, parts.join("\n\n"))
            .map_err(CmdError::internal)?;
        Ok(session.info())
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

    /// The browser's own agents may rename, close, edit or delete their own chat, never
    /// another agent's (people and the clients they connected may).
    pub(crate) fn refuse_other_agents_chat(
        &self,
        caller: &Caller,
        id: &str,
        what: &str,
    ) -> Result<(), CmdError> {
        if !self.is_own_agent(caller) || caller.agent_id() == Some(id) {
            return Ok(());
        }
        Err(CmdError::new(
            ErrorCode::NeedsApproval,
            format!("an AI agent cannot {what} another agent's chat"),
        )
        .with_hint("Only its own chat; ask the user to do it in the browser's AI panel."))
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
