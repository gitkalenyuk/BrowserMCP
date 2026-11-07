//! Approval requests of CLI agents ("may I run this command?") waiting for a person.
//!
//! A backend asks with [`Approvals::ask`]; the request is published as `ai.permission
//! {status: "pending"}` and stays listable (`ai.permissions_list`) until someone answers it
//! (`ai.permission_answer`, from the chat card or any API client), the agent withdraws it, or
//! the session stops. Every outcome is published as `ai.permission` with the final status, so
//! all open UIs drop the card.
//!
//! "Always allow" is remembered per session and tool: later requests for that tool are
//! allowed without asking.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;

/// What kind of action is waiting for approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// A Claude Code tool call (Bash, Edit, WebFetch…).
    Tool,
    /// A Codex shell command.
    Command,
    /// A Codex file change.
    FileChange,
}

/// A request as a backend makes it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewPermission {
    /// Tool name; also the key of "always allow" (`Bash`, `shell`, `file_change`).
    pub tool: String,
    pub kind: PermissionKind,
    /// One line for the card, when the agent gives one ("Claude wants to run ls").
    pub title: Option<String>,
    /// Why the agent asks, when it says.
    pub reason: Option<String>,
    /// What would run: tool input, `{command, cwd}`, `{files}`.
    pub input: Value,
}

/// Status of a request in `ai.permission` events and transcript entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Pending,
    Allowed,
    Denied,
    /// Withdrawn by the agent or ended with the session.
    Cancelled,
}

/// A request waiting for (or given) an answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRequest {
    /// Answer it with ai.permission_answer {id}.
    pub id: String,
    /// The AI session that asks.
    pub session: String,
    pub tool: String,
    pub kind: PermissionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Why the agent asks, if it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub input: Value,
    pub status: PermissionStatus,
    /// Allowed for the rest of the chat ("always allow this tool").
    #[serde(default)]
    pub always: bool,
    /// Denied: the reason the user gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// ms since the Unix epoch.
    pub created_ms: u64,
}

/// The answer a backend gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allow, and stop asking about this tool in this session.
    AllowAlways,
    Deny {
        reason: Option<String>,
    },
    /// Nobody will answer (the session stopped, or the agent withdrew the request).
    Cancelled,
}

impl Decision {
    /// Text the agent sees for a refusal.
    pub fn deny_message(&self) -> String {
        match self {
            Decision::Deny {
                reason: Some(reason),
            } if !reason.trim().is_empty() => {
                format!("The user denied this action: {}", reason.trim())
            }
            Decision::Cancelled => "The request was cancelled before the user answered.".into(),
            _ => "The user denied this action.".into(),
        }
    }
}

/// How [`Approvals::ask`] went.
pub enum Ask {
    /// Answered at once (the tool is always allowed).
    Answered(Decision),
    /// Published; the decision arrives on the receiver (an error = cancelled).
    Pending {
        request: Box<PermissionRequest>,
        answer: oneshot::Receiver<Decision>,
    },
}

struct Waiting {
    request: PermissionRequest,
    answer: oneshot::Sender<Decision>,
}

#[derive(Default)]
struct State {
    waiting: BTreeMap<String, Waiting>,
    always: BTreeSet<String>,
    counter: u64,
}

/// The approval requests of one session.
#[derive(Default)]
pub struct Approvals(Mutex<State>);

impl Approvals {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a request (or answer it right away when its tool is always allowed).
    pub fn ask(&self, session: &str, new: NewPermission, now_ms: u64) -> Ask {
        let mut state = self.lock();
        if state.always.contains(&new.tool) {
            return Ask::Answered(Decision::Allow);
        }
        state.counter += 1;
        let request = PermissionRequest {
            id: format!("{session}.p{}", state.counter),
            session: session.to_string(),
            tool: new.tool,
            kind: new.kind,
            title: new.title,
            reason: new.reason,
            input: new.input,
            status: PermissionStatus::Pending,
            always: false,
            answer: None,
            created_ms: now_ms,
        };
        let (tx, rx) = oneshot::channel();
        state.waiting.insert(
            request.id.clone(),
            Waiting {
                request: request.clone(),
                answer: tx,
            },
        );
        Ask::Pending {
            request: Box::new(request),
            answer: rx,
        }
    }

    /// Requests still waiting, oldest first.
    pub fn pending(&self) -> Vec<PermissionRequest> {
        let state = self.lock();
        let mut list: Vec<_> = state.waiting.values().map(|w| w.request.clone()).collect();
        list.sort_by_key(|r| r.created_ms);
        list
    }

    pub fn has(&self, id: &str) -> bool {
        self.lock().waiting.contains_key(id)
    }

    /// Answer `id`. Returns the resolved requests: `id` itself, plus (for "always") every
    /// other request for the same tool that was waiting.
    pub fn answer(&self, id: &str, decision: Decision) -> Option<Vec<PermissionRequest>> {
        let mut state = self.lock();
        let first = state.waiting.remove(id)?;
        let mut done = vec![(first, decision.clone())];
        if decision == Decision::AllowAlways {
            let tool = done[0].0.request.tool.clone();
            state.always.insert(tool.clone());
            let same: Vec<String> = state
                .waiting
                .iter()
                .filter(|(_, w)| w.request.tool == tool)
                .map(|(k, _)| k.clone())
                .collect();
            for key in same {
                if let Some(w) = state.waiting.remove(&key) {
                    done.push((w, Decision::Allow));
                }
            }
        }
        drop(state);
        Some(
            done.into_iter()
                .map(|(w, decision)| {
                    let request = resolved(w.request, &decision);
                    let _ = w.answer.send(decision);
                    request
                })
                .collect(),
        )
    }

    /// Withdraw one request (the agent no longer waits for it).
    pub fn cancel(&self, id: &str) -> Option<PermissionRequest> {
        let waiting = self.lock().waiting.remove(id)?;
        let _ = waiting.answer.send(Decision::Cancelled);
        Some(resolved(waiting.request, &Decision::Cancelled))
    }

    /// Withdraw every request (the session stopped).
    pub fn cancel_all(&self) -> Vec<PermissionRequest> {
        let waiting = std::mem::take(&mut self.lock().waiting);
        waiting
            .into_values()
            .map(|w| {
                let _ = w.answer.send(Decision::Cancelled);
                resolved(w.request, &Decision::Cancelled)
            })
            .collect()
    }
}

fn resolved(mut request: PermissionRequest, decision: &Decision) -> PermissionRequest {
    request.status = match decision {
        Decision::Allow | Decision::AllowAlways => PermissionStatus::Allowed,
        Decision::Deny { .. } => PermissionStatus::Denied,
        Decision::Cancelled => PermissionStatus::Cancelled,
    };
    request.always = *decision == Decision::AllowAlways;
    if let Decision::Deny { reason } = decision {
        request.answer = reason.clone().filter(|r| !r.trim().is_empty());
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(cmd: &str) -> NewPermission {
        NewPermission {
            tool: "Bash".into(),
            kind: PermissionKind::Tool,
            title: None,
            reason: None,
            input: json!({ "command": cmd }),
        }
    }

    fn pending(ask: Ask) -> (PermissionRequest, oneshot::Receiver<Decision>) {
        match ask {
            Ask::Pending { request, answer } => (*request, answer),
            Ask::Answered(d) => panic!("answered at once: {d:?}"),
        }
    }

    #[tokio::test]
    async fn answer_deny_and_always() {
        let approvals = Approvals::default();
        let (a, rx_a) = pending(approvals.ask("s", bash("ls"), 1));
        let (b, rx_b) = pending(approvals.ask("s", bash("dir"), 2));
        assert_eq!(a.id, "s.p1");
        assert_eq!(approvals.pending().len(), 2);

        let done = approvals
            .answer(
                &a.id,
                Decision::Deny {
                    reason: Some("not now".into()),
                },
            )
            .unwrap();
        assert_eq!(done[0].status, PermissionStatus::Denied);
        assert_eq!(done[0].answer.as_deref(), Some("not now"));
        let decision = rx_a.await.unwrap();
        assert_eq!(
            decision.deny_message(),
            "The user denied this action: not now"
        );
        assert!(
            approvals.answer(&a.id, Decision::Allow).is_none(),
            "answered once"
        );

        // A third request for the same tool, then "always" on the second: both resolve, and
        // later requests do not ask.
        let (c, rx_c) = pending(approvals.ask("s", bash("pwd"), 3));
        let done = approvals.answer(&b.id, Decision::AllowAlways).unwrap();
        assert_eq!(done.len(), 2);
        assert!(done[0].always);
        assert_eq!(done[1].id, c.id);
        assert_eq!(rx_b.await.unwrap(), Decision::AllowAlways);
        assert_eq!(rx_c.await.unwrap(), Decision::Allow);
        assert!(matches!(
            approvals.ask("s", bash("whoami"), 4),
            Ask::Answered(Decision::Allow)
        ));
        assert!(approvals.pending().is_empty());
    }

    #[tokio::test]
    async fn cancel_one_and_all() {
        let approvals = Approvals::default();
        let (a, rx_a) = pending(approvals.ask("s", bash("ls"), 1));
        let (_, rx_b) = pending(approvals.ask("s", bash("dir"), 2));
        assert_eq!(
            approvals.cancel(&a.id).unwrap().status,
            PermissionStatus::Cancelled
        );
        assert_eq!(rx_a.await.unwrap(), Decision::Cancelled);
        let gone = approvals.cancel_all();
        assert_eq!(gone.len(), 1);
        assert_eq!(rx_b.await.unwrap(), Decision::Cancelled);
        assert!(approvals.cancel_all().is_empty());
    }
}
