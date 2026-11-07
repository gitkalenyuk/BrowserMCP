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
//! * `ai.session {session, status}` — status changes (`running`, `idle`, `stopped`, `failed`);
//! * `ai.permission {session, id, status, tool, …}` — a CLI agent asks before a tool call
//!   (`pending`) and how that ended (`allowed`, `denied`, `cancelled`), see [`crate::cli::permission`];
//! * `ai.session_closed {session}` — removed with `ai.session_close`.
//!
//! A session outlives its backend: when the agent stopped or failed, the next message starts
//! a new backend task for the SAME session ([`Session::restart`]), which continues the
//! conversation (Claude Code `--resume`, Codex `thread/resume`, API agents keep their history).
//!
//! On disk (`data/ai/sessions/`): `<id>.jsonl` (transcript, appended), `<id>.meta.json` (what
//! is needed to list and continue the session after a browser restart) and, for API agents,
//! `<id>.history.json` (the model conversation).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bmcp_core::EventBus;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::cli::access::Access;
use crate::cli::permission::{Approvals, Ask, Decision, NewPermission, PermissionRequest};
use crate::model::Message;

/// Event topics published by this crate.
pub mod topics {
    pub const MESSAGE: &str = "ai.message";
    pub const TOOL_CALL: &str = "ai.tool_call";
    pub const TOOL_RESULT: &str = "ai.tool_result";
    pub const DONE: &str = "ai.done";
    pub const SESSION: &str = "ai.session";
    pub const PERMISSION: &str = "ai.permission";
    pub const SESSION_CLOSED: &str = "ai.session_closed";
    pub const TERM_OUTPUT: &str = "term.output";
    pub const TERM_EXIT: &str = "term.exit";
}

/// Tab-group colors handed out to agents in turn.
pub const COLORS: &[&str] = &[
    "#3b82f6", "#f97316", "#22c55e", "#a855f7", "#ec4899", "#14b8a6", "#eab308", "#ef4444",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Working on a user message.
    Running,
    /// Waiting for the next message (`ai.session_send`).
    Idle,
    /// Ended by `ai.session_stop` (or the CLI exited normally). The next message continues it.
    Stopped,
    /// Could not start or crashed. The next message tries to continue it.
    Failed,
}

impl Status {
    /// No backend runs: the next message has to start one.
    pub fn ended(self) -> bool {
        matches!(self, Status::Stopped | Status::Failed)
    }
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
    /// A note the UI knows and shows in the user's language: `code` names it, `params` fill
    /// it in, `text` is the English text for other clients (`ai.message {role: system, text,
    /// code, params}`).
    KnownNotice {
        code: &'static str,
        params: Value,
        text: String,
    },
    /// The CLI's own id of this conversation (Claude Code session id, Codex thread id): kept
    /// (and saved) so a later message can resume it.
    ConversationId(String),
}

impl AgentEvent {
    /// The CLI could not continue its earlier conversation, so the chat goes on in a new one
    /// (`agent`: "Claude Code", "Codex").
    pub fn resume_lost(agent: &str) -> Self {
        AgentEvent::KnownNotice {
            code: "resume_lost",
            params: json!({ "agent": agent }),
            text: format!(
                "{agent} could not continue its earlier conversation, so this chat goes on in a \
                 new one: the agent does not see the messages above."
            ),
        }
    }

    /// The CLI agent did not connect to the browser's MCP server: it has no browser tools.
    pub fn mcp_not_connected(agent: &str, status: &str) -> Self {
        AgentEvent::KnownNotice {
            code: "mcp_not_connected",
            params: json!({ "agent": agent, "status": status }),
            text: format!(
                "{agent} could not connect to the BrowserMCP MCP server (status: {status})"
            ),
        }
    }
}

/// How the session was started; reused when it is continued.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    /// Toolset names (`core`, `net`…).
    pub toolsets: Vec<String>,
    /// API agents: model calls per user message.
    pub max_steps: u32,
    /// CLI agents: permissions and folders.
    pub access: Option<Access>,
}

impl Default for Launch {
    fn default() -> Self {
        Launch {
            toolsets: vec!["core".into()],
            max_steps: 30,
            access: None,
        }
    }
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
    /// CLI agents: permission mode, sandbox and working folder in effect.
    pub access: Option<Access>,
    /// Approval requests waiting for an answer (see ai.permissions_list).
    pub pending_permissions: usize,
}

/// `<id>.meta.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: String,
    pub provider: String,
    pub model: Option<String>,
    pub color: String,
    pub created_ms: u64,
    pub launch: Launch,
    /// See [`AgentEvent::ConversationId`].
    #[serde(default)]
    pub conversation: Option<String>,
}

struct State {
    status: Status,
    tabs: BTreeSet<i32>,
    transcript: Vec<Value>,
    conversation: Option<String>,
}

/// The current backend run: its message queue, cancel token and task.
struct Run {
    input: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    /// Counts restarts: lets the end of a run tell whether a newer run took over.
    generation: u64,
}

pub struct Session {
    pub id: String,
    pub provider: String,
    pub model: Option<String>,
    pub color: String,
    pub created_ms: u64,
    pub launch: Launch,
    /// Replaced on restart: a session restored from disk gets the live bus then.
    events: Mutex<EventBus>,
    state: Mutex<State>,
    run: Mutex<Run>,
    /// Held while the session is continued (or closed): two messages sent at once to an ended
    /// chat (UI + MCP, two windows) must start ONE backend, not two fighting over it.
    lifecycle: tokio::sync::Mutex<()>,
    /// Set by `ai.session_close`: a send that raced with it must not bring it back.
    closed: AtomicBool,
    /// Approval requests of CLI agents.
    approvals: Approvals,
    /// API agents: the model conversation, kept across restarts of the agent loop.
    api_history: Mutex<Vec<Message>>,
    /// `data/ai/sessions/<id>.jsonl`; the meta and history files sit next to it.
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
            launch: Launch::default(),
            events: Mutex::new(events),
            state: Mutex::new(State {
                status: Status::Idle,
                tabs: BTreeSet::new(),
                transcript: Vec::new(),
                conversation: None,
            }),
            run: Mutex::new(Run {
                input,
                cancel: CancellationToken::new(),
                task: None,
                generation: 0,
            }),
            lifecycle: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            approvals: Approvals::default(),
            api_history: Mutex::new(Vec::new()),
            log_path,
        };
        (session, rx)
    }

    /// A session saved by an earlier run of the browser: stopped, transcript and history read
    /// back from disk. It continues (with [`Session::restart`]) when it gets a message.
    pub fn restore(saved: SavedSession, log_path: PathBuf) -> Self {
        let (mut session, _rx) = Session::new(
            saved.id,
            saved.provider,
            saved.model,
            saved.color,
            EventBus::new(),
            Some(log_path.clone()),
        );
        session.created_ms = saved.created_ms;
        session.launch = saved.launch;
        session
            .run
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .cancel
            .cancel();
        let state = session.state.get_mut().unwrap_or_else(|e| e.into_inner());
        state.status = Status::Stopped;
        state.conversation = saved.conversation;
        state.transcript = read_transcript(&log_path);
        // Tabs are not restored: tab ids do not survive a browser restart.
        // Without a saved model conversation (an older session), the transcript's text messages
        // are the history. Done here, once, and never from the live transcript: by the time an
        // agent loop reads its history, the message it is about to send is already recorded.
        let history = read_json::<Vec<Message>>(&with_suffix(&log_path, "history.json"))
            .unwrap_or_else(|| history_from_transcript(&state.transcript));
        *session
            .api_history
            .get_mut()
            .unwrap_or_else(|e| e.into_inner()) = history;
        session
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(&self) -> std::sync::MutexGuard<'_, Run> {
        self.run.lock().unwrap_or_else(|e| e.into_inner())
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
            access: self.launch.access.clone(),
            pending_permissions: self.approvals.pending().len(),
        }
    }

    pub fn status(&self) -> Status {
        self.lock().status
    }

    pub fn transcript(&self) -> Vec<Value> {
        self.lock().transcript.clone()
    }

    /// The CLI's own conversation id, once the backend reported it.
    pub fn conversation(&self) -> Option<String> {
        self.lock().conversation.clone()
    }

    // ------------------------------------------------------------ backend run

    /// Cancelled by `ai.session_stop`; a new token comes with every restart.
    pub fn cancel_token(&self) -> CancellationToken {
        self.run().cancel.clone()
    }

    /// Stop the current backend run.
    pub fn cancel(&self) {
        self.run().cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.run().cancel.is_cancelled()
    }

    /// The backend has ended (or was stopped): the next message must restart it first.
    pub fn needs_restart(&self) -> bool {
        self.status().ended() || self.is_cancelled()
    }

    /// Which run is current (see [`Session::restart`]).
    pub fn generation(&self) -> u64 {
        self.run().generation
    }

    /// Taken around continuing or closing the session (see `lifecycle`).
    pub async fn lifecycle(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.lifecycle.lock().await
    }

    pub fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Remember the task running the backend (awaited before a restart).
    pub fn set_task(&self, task: JoinHandle<()>) {
        self.run().task = Some(task);
    }

    /// The task of the last run, if any (taken: the caller waits for it).
    pub fn take_task(&self) -> Option<JoinHandle<()>> {
        self.run().task.take()
    }

    /// Prepare a new backend run of this ended session: a fresh message queue and cancel
    /// token, and the live event bus. The previous task must have finished (see
    /// [`Session::take_task`]), or both would report into this session.
    pub fn restart(&self, events: EventBus) -> mpsc::UnboundedReceiver<String> {
        *self.events.lock().unwrap_or_else(|e| e.into_inner()) = events;
        let (input, rx) = mpsc::unbounded_channel();
        {
            let mut run = self.run();
            run.input = input;
            run.cancel = CancellationToken::new();
            run.generation += 1;
        }
        self.set_status(Status::Idle);
        rx
    }

    /// Queue a user message; records and announces it. Fails when the session has ended
    /// (`AiService::send` restarts it first).
    pub fn send(&self, text: String) -> Result<(), String> {
        if self.needs_restart() {
            return Err(format!("session {} has ended", self.id));
        }
        self.record(json!({ "type": "user", "text": text }));
        self.emit(topics::MESSAGE, json!({ "role": "user", "text": text }));
        self.run()
            .input
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
            AgentEvent::KnownNotice { code, params, text } => {
                self.record(
                    json!({ "type": "notice", "text": text, "code": code, "params": params }),
                );
                self.emit(
                    topics::MESSAGE,
                    json!({ "role": "system", "text": text, "code": code, "params": params }),
                );
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
                // The status changes BEFORE `ai.done` goes out: a client that sends the next
                // message as soon as it sees `ai.done` must find the session idle, not busy.
                self.set_status(if self.is_cancelled() {
                    Status::Stopped
                } else {
                    Status::Idle
                });
                self.emit(topics::DONE, json!({ "reason": reason }));
            }
            AgentEvent::ConversationId(id) => {
                let changed = {
                    let mut state = self.lock();
                    let changed = state.conversation.as_deref() != Some(id.as_str());
                    state.conversation = Some(id);
                    changed
                };
                if changed {
                    self.save_meta();
                }
            }
        }
    }

    // ------------------------------------------------------------ permissions

    /// A CLI agent asks before a tool call. Publishes `ai.permission {status: pending}` unless
    /// the tool is always allowed in this session.
    pub fn ask_permission(&self, new: NewPermission) -> Ask {
        let ask = self.approvals.ask(&self.id, new, now_ms());
        if let Ask::Pending { request, .. } = &ask {
            self.emit(topics::PERMISSION, permission_event(request));
        }
        ask
    }

    /// Requests still waiting for an answer.
    pub fn pending_permissions(&self) -> Vec<PermissionRequest> {
        self.approvals.pending()
    }

    pub fn has_permission(&self, id: &str) -> bool {
        self.approvals.has(id)
    }

    /// Answer a request (from the chat card or `ai.permission_answer`).
    pub fn answer_permission(
        &self,
        id: &str,
        decision: Decision,
    ) -> Option<Vec<PermissionRequest>> {
        let done = self.approvals.answer(id, decision)?;
        done.iter().for_each(|r| self.publish_resolved(r));
        Some(done)
    }

    /// The agent withdrew a request.
    pub fn cancel_permission(&self, id: &str) {
        if let Some(request) = self.approvals.cancel(id) {
            self.publish_resolved(&request);
        }
    }

    /// The backend ended: nobody will wait for answers any more.
    pub fn cancel_permissions(&self) {
        for request in self.approvals.cancel_all() {
            self.publish_resolved(&request);
        }
    }

    fn publish_resolved(&self, request: &PermissionRequest) {
        let mut entry = serde_json::to_value(request).unwrap_or_default();
        entry["type"] = json!("permission");
        self.record(entry);
        self.emit(topics::PERMISSION, permission_event(request));
    }

    // ------------------------------------------------------------ API history

    /// API agents: the model conversation so far (empty for a new chat; see
    /// [`Session::restore`] for a chat saved without one).
    pub fn api_history(&self) -> Vec<Message> {
        self.api_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Keep (and save) the API conversation, made consistent for the next request.
    pub fn set_api_history(&self, mut history: Vec<Message>) {
        repair_history(&mut history);
        if let Some(log) = &self.log_path
            && let Err(err) = write_json(&with_suffix(log, "history.json"), &history)
        {
            tracing::warn!(%err, "cannot save the AI conversation");
        }
        *self.api_history.lock().unwrap_or_else(|e| e.into_inner()) = history;
    }

    // ------------------------------------------------------------ files

    /// What `<id>.meta.json` holds.
    pub fn saved(&self) -> SavedSession {
        SavedSession {
            id: self.id.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            color: self.color.clone(),
            created_ms: self.created_ms,
            launch: self.launch.clone(),
            conversation: self.conversation(),
        }
    }

    /// Write `<id>.meta.json` (the session is listed again after a browser restart).
    pub fn save_meta(&self) {
        if let Some(log) = &self.log_path
            && let Err(err) = write_json(&meta_path(log), &self.saved())
        {
            tracing::warn!(%err, "cannot save the AI session");
        }
    }

    /// Delete the files of a closed session: always the meta file (it is not listed again),
    /// the transcript and history only when asked.
    pub fn remove_files(&self, transcript_too: bool) {
        let Some(log) = &self.log_path else { return };
        let _ = std::fs::remove_file(meta_path(log));
        if transcript_too {
            let _ = std::fs::remove_file(log);
            let _ = std::fs::remove_file(with_suffix(log, "history.json"));
        }
    }

    // ------------------------------------------------------------ internals

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
        let events = self
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        events.emit(topic, data);
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

/// `ai.permission` payload (without `session`, which `emit` adds).
fn permission_event(request: &PermissionRequest) -> Value {
    let mut data = serde_json::to_value(request).unwrap_or_default();
    if let Some(map) = data.as_object_mut() {
        map.remove("session");
    }
    data
}

/// Every tool call of the model needs a result before the next request (the providers
/// reject the conversation otherwise). A run stopped mid-step leaves calls without one: give
/// them an error result, so the conversation can go on.
pub fn repair_history(history: &mut Vec<Message>) {
    let mut i = 0;
    while i < history.len() {
        if let Message::Assistant { calls, .. } = &history[i] {
            let calls = calls.clone();
            let mut at = i + 1;
            for call in calls {
                let answered = history[i + 1..]
                    .iter()
                    .take_while(|m| matches!(m, Message::Tool { .. }))
                    .any(|m| matches!(m, Message::Tool { call_id, .. } if *call_id == call.id));
                if !answered {
                    // After the results that are there, before the next user message.
                    while at < history.len() && matches!(history[at], Message::Tool { .. }) {
                        at += 1;
                    }
                    history.insert(
                        at,
                        Message::Tool {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            output: "Stopped by the user before this tool ran.".into(),
                            is_error: true,
                        },
                    );
                }
            }
        }
        i += 1;
    }
}

/// The text messages of a transcript as a model conversation (tool calls are not in the
/// transcript in a replayable form). Consecutive messages of one role are joined.
pub fn history_from_transcript(entries: &[Value]) -> Vec<Message> {
    let mut history: Vec<Message> = Vec::new();
    for entry in entries {
        let Some(text) = entry["text"].as_str() else {
            continue;
        };
        match (entry["type"].as_str(), history.last_mut()) {
            (Some("user"), Some(Message::User { text: last })) => {
                last.push_str("\n\n");
                last.push_str(text);
            }
            (Some("user"), _) => history.push(Message::User { text: text.into() }),
            (Some("assistant"), Some(Message::Assistant { text: last, .. })) => {
                last.push_str("\n\n");
                last.push_str(text);
            }
            // The conversation must start with the user.
            (Some("assistant"), Some(_)) => history.push(Message::Assistant {
                text: text.into(),
                calls: Vec::new(),
                raw: None,
            }),
            _ => {}
        }
    }
    history
}

fn with_suffix(log: &Path, suffix: &str) -> PathBuf {
    log.with_extension(suffix)
}

fn meta_path(log: &Path) -> PathBuf {
    with_suffix(log, "meta.json")
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

fn read_transcript(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Sessions saved in `dir` (`*.meta.json`), oldest first.
pub fn saved_sessions(dir: &Path) -> Vec<(SavedSession, PathBuf)> {
    let mut found: Vec<(SavedSession, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".meta.json"))
        .filter_map(|p| {
            let saved: SavedSession = read_json(&p)?;
            let log = dir.join(format!("{}.jsonl", saved.id));
            Some((saved, log))
        })
        .collect();
    found.sort_by_key(|(s, _)| s.created_ms);
    found
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
    use crate::cli::permission::{PermissionKind, PermissionStatus};
    use crate::model::ToolCall;

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

    /// A client that reacts to `ai.done` by sending the next message must find the session idle.
    #[tokio::test]
    async fn the_session_is_idle_before_ai_done_is_published() {
        let bus = EventBus::new();
        let mut rx_bus = bus.subscribe();
        let (session, _rx) = Session::new(
            "custom-1".into(),
            "custom".into(),
            None,
            COLORS[0].into(),
            bus,
            None,
        );
        session.set_status(Status::Running);
        session.apply(AgentEvent::Done("end_turn".into()));
        let events: Vec<_> = std::iter::from_fn(|| rx_bus.try_recv().ok()).collect();
        let done = events.iter().position(|e| e.topic == "ai.done").unwrap();
        let idle = events
            .iter()
            .position(|e| e.topic == "ai.session" && e.data["status"] == "idle")
            .expect("an ai.session idle event");
        assert!(idle < done, "status must change before ai.done");
        assert_eq!(session.status(), Status::Idle);

        // A stopped session ends as stopped, not idle.
        session.set_status(Status::Running);
        session.cancel();
        session.apply(AgentEvent::Done("stopped".into()));
        assert_eq!(session.status(), Status::Stopped);
    }

    #[tokio::test]
    async fn restart_gives_an_ended_session_a_new_queue() {
        let (session, rx) = Session::new(
            "claude-code-1".into(),
            "claude-code".into(),
            None,
            COLORS[0].into(),
            EventBus::new(),
            None,
        );
        drop(rx);
        session.cancel();
        session.set_status(Status::Stopped);
        assert!(session.send("x".into()).is_err());

        let old = session.cancel_token();
        let mut rx = session.restart(EventBus::new());
        assert!(old.is_cancelled() && !session.is_cancelled());
        assert_eq!(session.status(), Status::Idle);
        session.send("again".into()).unwrap();
        assert_eq!(rx.recv().await.unwrap(), "again");
        assert_eq!(session.status(), Status::Running);
    }

    #[tokio::test]
    async fn meta_and_transcript_restore_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("claude-code-4.jsonl");
        let (mut session, _rx) = Session::new(
            "claude-code-4".into(),
            "claude-code".into(),
            Some("sonnet".into()),
            COLORS[1].into(),
            EventBus::new(),
            Some(log.clone()),
        );
        session.launch.toolsets = vec!["core".into(), "net".into()];
        session.save_meta();
        session.send("hello".into()).unwrap();
        session.apply(AgentEvent::ConversationId("5f0c".into()));
        session.apply(AgentEvent::Text("hi!".into()));

        let saved = saved_sessions(dir.path());
        assert_eq!(saved.len(), 1);
        let (meta, path) = saved.into_iter().next().unwrap();
        assert_eq!(meta.conversation.as_deref(), Some("5f0c"));
        let restored = Session::restore(meta, path);
        assert_eq!(restored.status(), Status::Stopped);
        assert_eq!(restored.model.as_deref(), Some("sonnet"));
        assert_eq!(restored.launch.toolsets, ["core", "net"]);
        assert_eq!(restored.conversation().as_deref(), Some("5f0c"));
        assert_eq!(restored.transcript().len(), 2);
        // Without a saved model conversation, the text messages are the history.
        assert_eq!(
            restored.api_history(),
            vec![
                Message::User {
                    text: "hello".into()
                },
                Message::Assistant {
                    text: "hi!".into(),
                    calls: vec![],
                    raw: None
                }
            ]
        );

        restored.remove_files(false);
        assert!(saved_sessions(dir.path()).is_empty());
        assert!(log.exists(), "the transcript stays unless asked");
        restored.remove_files(true);
        assert!(!log.exists());
    }

    #[tokio::test]
    async fn permissions_are_published_recorded_and_cancelled() {
        let bus = EventBus::new();
        let mut rx_bus = bus.subscribe();
        let (session, _rx) = Session::new(
            "claude-code-1".into(),
            "claude-code".into(),
            None,
            COLORS[0].into(),
            bus,
            None,
        );
        let new = || NewPermission {
            tool: "Bash".into(),
            kind: PermissionKind::Tool,
            title: Some("Run ls".into()),
            reason: None,
            input: json!({"command": "ls"}),
        };
        let Ask::Pending { request, answer } = session.ask_permission(new()) else {
            panic!("must ask")
        };
        assert_eq!(session.info().pending_permissions, 1);
        let pending = rx_bus.try_recv().unwrap();
        assert_eq!(pending.topic, "ai.permission");
        assert_eq!(pending.data["status"], "pending");
        assert_eq!(pending.data["session"], "claude-code-1");
        assert_eq!(pending.data["input"]["command"], "ls");

        session
            .answer_permission(
                &request.id,
                Decision::Deny {
                    reason: Some("no".into()),
                },
            )
            .unwrap();
        assert!(matches!(answer.await, Ok(Decision::Deny { .. })));
        let denied = rx_bus.try_recv().unwrap();
        assert_eq!(denied.data["status"], "denied");
        assert_eq!(denied.data["answer"], "no");
        let entry = session.transcript().pop().unwrap();
        assert_eq!(entry["type"], "permission");
        assert_eq!(entry["status"], "denied");

        let Ask::Pending { answer, .. } = session.ask_permission(new()) else {
            panic!("must ask")
        };
        session.cancel_permissions();
        assert_eq!(answer.await.unwrap(), Decision::Cancelled);
        assert!(session.pending_permissions().is_empty());
        let last = std::iter::from_fn(|| rx_bus.try_recv().ok())
            .last()
            .unwrap();
        assert_eq!(last.data["status"], json!(PermissionStatus::Cancelled));
    }

    #[test]
    fn repair_answers_tool_calls_left_without_results() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "tabs_open".into(),
            args: json!({}),
        };
        let tool = |id: &str| Message::Tool {
            call_id: id.into(),
            name: "tabs_open".into(),
            output: "{}".into(),
            is_error: false,
        };
        let mut history = vec![
            Message::User { text: "go".into() },
            Message::Assistant {
                text: String::new(),
                calls: vec![call("a"), call("b")],
                raw: None,
            },
            tool("a"),
            Message::User {
                text: "again".into(),
            },
        ];
        repair_history(&mut history);
        assert_eq!(history.len(), 5);
        assert!(
            matches!(&history[3], Message::Tool { call_id, is_error: true, .. } if call_id == "b")
        );
        assert!(matches!(history[4], Message::User { .. }));
        // Already consistent: unchanged.
        let before = history.clone();
        repair_history(&mut history);
        assert_eq!(history, before);
    }
}
