//! One AI session = one agent: id, color, status, transcript, the tabs it touched.
//!
//! Every backend (API agent loop, Claude Code, Codex) reports what happens as [`AgentEvent`]s;
//! [`Session::apply`] turns them into EventBus events, transcript lines and tab ownership, so
//! the UI sees the same `ai.*` events whatever drives the agent:
//!
//! * `ai.message {session, role, text_delta}` — streamed text; `{session, role, text}` — whole message;
//! * `ai.tool_call {session, name, args}`;
//! * `ai.tool_result {session, name, ok, summary}`;
//! * `ai.done {session, reason}` — the agent finished answering one user message;
//! * `ai.session {session, status}` — status changes (`running`, `idle`, `stopped`, `failed`).
//!
//! The transcript is kept in memory and appended to `data/ai/sessions/<id>.jsonl`.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use bmcp_core::EventBus;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Event topics published by this crate.
pub mod topics {
    pub const MESSAGE: &str = "ai.message";
    pub const TOOL_CALL: &str = "ai.tool_call";
    pub const TOOL_RESULT: &str = "ai.tool_result";
    pub const DONE: &str = "ai.done";
    pub const SESSION: &str = "ai.session";
    pub const TERM_OUTPUT: &str = "term.output";
    pub const TERM_EXIT: &str = "term.exit";
}

/// Tab-group colors handed out to agents in turn.
pub const COLORS: &[&str] = &[
    "#3b82f6", "#f97316", "#22c55e", "#a855f7", "#ec4899", "#14b8a6", "#eab308", "#ef4444",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Working on a user message.
    Running,
    /// Waiting for the next message (`ai.session_send`).
    Idle,
    /// Ended by `ai.session_stop` (or the CLI exited normally).
    Stopped,
    /// Could not start or crashed.
    Failed,
}

/// Something that happened in a session, independent of the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A chunk of streamed assistant text.
    TextDelta(String),
    /// A complete assistant message.
    Text(String),
    ToolCall {
        name: String,
        args: Value,
    },
    ToolResult {
        name: String,
        ok: bool,
        summary: String,
        /// Full output when known (used to find tab ids).
        output: Option<Value>,
    },
    /// The answer to one user message is complete (`end_turn`, `max_steps`, `error: …`).
    Done(String),
    /// A note for the user (warnings, CLI stderr…), shown as role `system`.
    Notice(String),
}

/// Public summary for `ai.sessions_list`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SessionInfo {
    /// Session id; also the agent id (`Caller::Agent { id }`, MCP `?agent=`).
    pub id: String,
    /// Provider id, e.g. `anthropic`, `claude-code`.
    pub provider: String,
    pub model: Option<String>,
    /// Tab-group color of this agent, `#rrggbb`.
    pub color: String,
    pub status: Status,
    /// Tabs this agent opened or acted on.
    pub tabs: Vec<i32>,
    /// Creation time, ms since the Unix epoch.
    pub created_ms: u64,
}

struct State {
    status: Status,
    tabs: BTreeSet<i32>,
    transcript: Vec<Value>,
}

pub struct Session {
    pub id: String,
    pub provider: String,
    pub model: Option<String>,
    pub color: String,
    pub created_ms: u64,
    events: EventBus,
    state: Mutex<State>,
    /// User messages for the running backend task.
    input: mpsc::UnboundedSender<String>,
    /// Cancelled by `ai.session_stop`.
    pub cancel: CancellationToken,
    log_path: Option<PathBuf>,
}

impl Session {
    /// A new session and the receiving end of its message queue (for the backend task).
    pub fn new(
        id: String,
        provider: String,
        model: Option<String>,
        color: String,
        events: EventBus,
        log_path: Option<PathBuf>,
    ) -> (Self, mpsc::UnboundedReceiver<String>) {
        let (input, rx) = mpsc::unbounded_channel();
        let session = Session {
            id,
            provider,
            model,
            color,
            created_ms: now_ms(),
            events,
            state: Mutex::new(State {
                status: Status::Idle,
                tabs: BTreeSet::new(),
                transcript: Vec::new(),
            }),
            input,
            cancel: CancellationToken::new(),
            log_path,
        };
        (session, rx)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn info(&self) -> SessionInfo {
        let state = self.lock();
        SessionInfo {
            id: self.id.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            color: self.color.clone(),
            status: state.status,
            tabs: state.tabs.iter().copied().collect(),
            created_ms: self.created_ms,
        }
    }

    pub fn status(&self) -> Status {
        self.lock().status
    }

    pub fn transcript(&self) -> Vec<Value> {
        self.lock().transcript.clone()
    }

    /// Queue a user message; records and announces it. Fails when the session has ended.
    pub fn send(&self, text: String) -> Result<(), String> {
        if matches!(self.status(), Status::Stopped | Status::Failed) || self.cancel.is_cancelled() {
            return Err(format!("session {} has ended", self.id));
        }
        self.record(json!({ "type": "user", "text": text }));
        self.emit(topics::MESSAGE, json!({ "role": "user", "text": text }));
        self.input
            .send(text)
            .map_err(|_| format!("session {} is not accepting messages", self.id))?;
        // The backend picks it up right away (or after the current answer).
        self.set_status(Status::Running);
        Ok(())
    }

    pub fn set_status(&self, status: Status) {
        let changed = {
            let mut state = self.lock();
            let changed = state.status != status;
            state.status = status;
            changed
        };
        if changed {
            self.emit(topics::SESSION, json!({ "status": status }));
        }
    }

    /// Publish an event for what happened and remember it in the transcript.
    pub fn apply(&self, event: AgentEvent) {
        match event {
            AgentEvent::TextDelta(delta) => {
                self.emit(
                    topics::MESSAGE,
                    json!({ "role": "assistant", "text_delta": delta }),
                );
            }
            AgentEvent::Text(text) => {
                self.record(json!({ "type": "assistant", "text": text }));
                self.emit(
                    topics::MESSAGE,
                    json!({ "role": "assistant", "text": text }),
                );
            }
            AgentEvent::Notice(text) => {
                self.record(json!({ "type": "notice", "text": text }));
                self.emit(topics::MESSAGE, json!({ "role": "system", "text": text }));
            }
            AgentEvent::ToolCall { name, args } => {
                self.touch_tabs(&args);
                self.record(json!({ "type": "tool_call", "name": name, "args": args }));
                self.emit(topics::TOOL_CALL, json!({ "name": name, "args": args }));
            }
            AgentEvent::ToolResult {
                name,
                ok,
                summary,
                output,
            } => {
                if ok && let Some(output) = &output {
                    self.touch_output_tabs(&name, output);
                }
                self.record(
                    json!({ "type": "tool_result", "name": name, "ok": ok, "summary": summary }),
                );
                self.emit(
                    topics::TOOL_RESULT,
                    json!({ "name": name, "ok": ok, "summary": summary }),
                );
            }
            AgentEvent::Done(reason) => {
                self.record(json!({ "type": "done", "reason": reason }));
                self.emit(topics::DONE, json!({ "reason": reason }));
            }
        }
    }

    /// `tab` in the arguments of a tool call.
    fn touch_tabs(&self, args: &Value) {
        if let Some(tab) = args.get("tab").and_then(Value::as_i64) {
            self.lock().tabs.insert(tab as i32);
        }
    }

    /// Tab ids in a command result: `tab` (tabs.open, page.*), or `id` for navigation results
    /// (`nav.goto` returns the tab's info).
    fn touch_output_tabs(&self, name: &str, output: &Value) {
        let short = name.rsplit("__").next().unwrap_or(name); // `mcp__browsermcp__nav_goto`
        let mut found = output.get("tab").and_then(Value::as_i64);
        if found.is_none() && (short.starts_with("nav") || short.starts_with("tabs")) {
            found = output.get("id").and_then(Value::as_i64);
        }
        if let Some(tab) = found {
            self.lock().tabs.insert(tab as i32);
        }
    }

    /// Publish `topic` with `session` added to the payload.
    fn emit(&self, topic: &str, mut data: Value) {
        data["session"] = json!(self.id);
        self.events.emit(topic, data);
    }

    /// Append to the transcript (memory + jsonl file).
    fn record(&self, mut entry: Value) {
        entry["ts_ms"] = json!(now_ms());
        if let Some(path) = &self.log_path {
            let line = format!("{entry}\n");
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| f.write_all(line.as_bytes()));
            if let Err(err) = written {
                tracing::warn!(%err, path = %path.display(), "cannot write AI transcript");
            }
        }
        self.lock().transcript.push(entry);
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Short text for `ai.tool_result.summary`: a JSON result or error message cut to ~200 chars.
pub fn summarize(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    crate::model::truncate(&text.replace('\n', " "), 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apply_emits_events_records_transcript_and_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let mut rx_bus = bus.subscribe();
        let log = dir.path().join("s.jsonl");
        let (session, mut rx) = Session::new(
            "anthropic-1".into(),
            "anthropic".into(),
            None,
            COLORS[0].into(),
            bus,
            Some(log.clone()),
        );
        session.send("hi".into()).unwrap();
        assert_eq!(rx.recv().await.unwrap(), "hi");
        session.apply(AgentEvent::ToolCall {
            name: "nav_goto".into(),
            args: json!({"tab": 3, "url": "x"}),
        });
        session.apply(AgentEvent::ToolResult {
            name: "mcp__browsermcp__tabs_open".into(),
            ok: true,
            summary: "{}".into(),
            output: Some(json!({"tab": 7})),
        });
        session.apply(AgentEvent::Done("end_turn".into()));

        assert_eq!(session.info().tabs, vec![3, 7]);
        let topics: Vec<_> = std::iter::from_fn(|| rx_bus.try_recv().ok())
            .map(|e| (e.topic, e.data["session"].clone()))
            .collect();
        assert_eq!(topics[0], ("ai.message".to_string(), json!("anthropic-1")));
        assert_eq!(topics.last().unwrap().0, "ai.done");
        let lines = std::fs::read_to_string(log).unwrap();
        assert_eq!(lines.lines().count(), 4);
        assert_eq!(session.transcript().len(), 4);
    }
}
