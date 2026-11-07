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
//! * `ai.session {session, status, …SessionInfo}` — the whole [`SessionInfo`] whenever it
//!   changes: status (`running`, `idle`, `stopped`, `failed`), title, model, archived, queued;
//! * `ai.permission {session, id, status, tool, …}` — a CLI agent asks before a tool call
//!   (`pending`) and how that ended (`allowed`, `denied`, `cancelled`), see [`crate::cli::permission`];
//! * `ai.session_closed {session}` — archived with `ai.session_close` (or deleted): it leaves
//!   the open chats;
//! * `ai.session_deleted {session}` — removed for good with `ai.session_delete`;
//! * `ai.message_taken {session, index}` — the agent took user message `index`, which was sent
//!   while it was busy (its `ai.message` carried `queued: true`);
//! * `ai.session_rewound {session, index}` — `ai.session_edit` cut the transcript just before
//!   user message `index` (the edited text follows as a new user message).
//!
//! User messages are numbered per chat (`index`, 0-based, on `ai.message` and transcript
//! entries `{type: user}`): `ai.session_edit` names a message by it.
//!
//! A session outlives its backend: when the agent stopped or failed, the next message starts
//! a new backend task for the SAME session ([`Session::restart`]), which continues the
//! conversation (Claude Code `--resume`, Codex `thread/resume`, API agents keep their history).
//! Closing a chat archives it (files kept, listed with `ai.sessions_list {archived: true}`);
//! `ai.session_reopen` brings it back, `ai.session_delete` removes it and its files.
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
    pub const SESSION_DELETED: &str = "ai.session_deleted";
    pub const MESSAGE_TAKEN: &str = "ai.message_taken";
    pub const SESSION_REWOUND: &str = "ai.session_rewound";
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
    /// Claude Code: the uuid of the last conversation entry of the turn that is about to end
    /// (sent right before its [`AgentEvent::Done`], recorded on the `done` entry as `at`).
    /// `ai.session_edit` resumes the conversation at such a point (`--resume-session-at`).
    Checkpoint(String),
    /// Codex: the id of the turn that is about to end (sent right before its
    /// [`AgentEvent::Done`], recorded on the `done` entry as `turn`). Only a `done` with it
    /// ended a turn Codex has in its thread: `ai.session_edit` rolls back by these
    /// (`thread/rollback {numTurns}`), never by the `done`s of a `turn/start` that failed or
    /// of a run stopped before its thread started.
    Turn(String),
}

/// What the backend task of a session receives.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A user message. `index` is its number in the chat (see the module docs), `None` for
    /// text that is not a message of its own (sent again after a lost conversation). `queued`:
    /// sent while the agent was busy ([`Session::take`] once the agent has it).
    Message {
        text: String,
        index: Option<usize>,
        queued: bool,
    },
    /// Use this model from the next turn on (`None` = the CLI's / provider's default).
    SetModel(Option<String>),
    /// Stop the current turn, keep the chat (and the CLI process) alive.
    Interrupt,
}

impl Input {
    /// A message that is not queued and has no index (tests, replays).
    pub fn text(text: impl Into<String>) -> Self {
        Input::Message {
            text: text.into(),
            index: None,
            queued: false,
        }
    }
}

/// The longest automatic chat title, in characters (with the "…").
pub const TITLE_MAX: usize = 48;

/// The automatic title of a chat from its first user message: whitespace collapsed, cut to
/// [`TITLE_MAX`] characters with "…". `None` for a blank message.
pub fn auto_title(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= TITLE_MAX {
        return Some(collapsed);
    }
    let cut: String = collapsed.chars().take(TITLE_MAX - 1).collect();
    Some(format!("{}…", cut.trim_end()))
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
    /// Name to show: the user's (ai.session_rename), else the first user message
    /// (whitespace collapsed, ~48 chars with "…"), else the provider's name. Never the id.
    pub title: String,
    /// The user renamed the chat: the automatic title no longer replaces it.
    pub title_set: bool,
    /// Closed with ai.session_close: not among the open chats, listed with
    /// ai.sessions_list {archived: true}; ai.session_reopen brings it back.
    pub archived: bool,
    /// Last activity (message, rename, model change…), ms since the Unix epoch.
    pub updated_ms: u64,
    /// User messages sent while the agent was busy that it has not taken yet.
    pub queued: usize,
}

/// `<id>.meta.json`. Fields added later have defaults, so older files still load.
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
    /// The user's title (`title_set`) or the automatic one; "" = none yet.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub title_set: bool,
    #[serde(default)]
    pub archived: bool,
    /// 0 in files written before it existed.
    #[serde(default)]
    pub updated_ms: u64,
}

struct State {
    status: Status,
    tabs: BTreeSet<i32>,
    transcript: Vec<Value>,
    conversation: Option<String>,
    model: Option<String>,
    /// Set by ai.session_rename.
    title: Option<String>,
    /// From the first user message.
    auto_title: Option<String>,
    archived: bool,
    updated_ms: u64,
    /// User messages so far (the next one gets this index).
    users: usize,
    /// Indexes of user messages sent while the agent was busy, not taken yet.
    queued: Vec<usize>,
    /// See [`AgentEvent::Checkpoint`]: attached to the next `done` entry.
    checkpoint: Option<String>,
    /// See [`AgentEvent::Turn`]: attached to the next `done` entry.
    turn: Option<String>,
    /// One-shot instructions for the next backend run after ai.session_edit.
    resume_at: Option<String>,
    rollback: Option<u32>,
}

/// The current backend run: its message queue, cancel token and task.
struct Run {
    input: mpsc::UnboundedSender<Input>,
    cancel: CancellationToken,
    /// Cancelled by ai.session_interrupt: ends the current turn of an API agent.
    turn: CancellationToken,
    task: Option<JoinHandle<()>>,
    /// Counts restarts: lets the end of a run tell whether a newer run took over.
    generation: u64,
}

/// What [`Session::rewind`] found out about the conversation before the edited message.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rewind {
    /// Texts of all user messages before the edit, in order (for API histories).
    pub user_texts: Vec<String>,
    /// User messages after the last finished turn and before the edited one: the agent's
    /// conversation is rewound to that turn, so they are sent again with the new text.
    pub prefix: Vec<String>,
    /// Claude Code: the checkpoint of that last finished turn ([`AgentEvent::Checkpoint`]).
    pub resume_at: Option<String>,
    /// A turn before the edit ended (done entry), with or without a checkpoint.
    pub turns_before: bool,
    /// Codex turns ended after that last finished turn (`thread/rollback {numTurns}`): the
    /// `done` entries marked with [`AgentEvent::Turn`].
    pub turns_after: u32,
    /// The user and assistant texts before the edit (to seed a fresh conversation).
    pub earlier: Vec<(String, String)>,
}

pub struct Session {
    pub id: String,
    pub provider: String,
    /// The provider's display name: the title until the first message.
    pub provider_name: String,
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
    ) -> (Self, mpsc::UnboundedReceiver<Input>) {
        let (input, rx) = mpsc::unbounded_channel();
        let created_ms = now_ms();
        let session = Session {
            id,
            provider_name: provider.clone(),
            provider,
            color,
            created_ms,
            launch: Launch::default(),
            events: Mutex::new(events),
            state: Mutex::new(State {
                status: Status::Idle,
                tabs: BTreeSet::new(),
                transcript: Vec::new(),
                conversation: None,
                model: model.filter(|m| !m.is_empty()),
                title: None,
                auto_title: None,
                archived: false,
                updated_ms: created_ms,
                users: 0,
                queued: Vec::new(),
                checkpoint: None,
                turn: None,
                resume_at: None,
                rollback: None,
            }),
            run: Mutex::new(Run {
                input,
                cancel: CancellationToken::new(),
                turn: CancellationToken::new(),
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
        // Older files: no title, archive flag or activity time (defaults), and user entries
        // without an index (numbered here, in order).
        let mut users = 0;
        for entry in state.transcript.iter_mut() {
            if entry["type"] == "user" {
                entry["index"] = json!(users);
                users += 1;
            }
        }
        state.users = users;
        state.archived = saved.archived;
        let saved_title = Some(saved.title.trim().to_string()).filter(|t| !t.is_empty());
        if saved.title_set {
            state.title = saved_title.clone();
        }
        state.auto_title = state
            .transcript
            .iter()
            .find(|e| e["type"] == "user")
            .and_then(|e| e["text"].as_str())
            .and_then(auto_title)
            .or(if saved.title_set { None } else { saved_title });
        let last_entry = state
            .transcript
            .iter()
            .filter_map(|e| e["ts_ms"].as_u64())
            .max()
            .unwrap_or(0);
        state.updated_ms = saved.updated_ms.max(last_entry).max(session.created_ms);
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
        let pending_permissions = self.approvals.pending().len();
        let state = self.lock();
        SessionInfo {
            id: self.id.clone(),
            provider: self.provider.clone(),
            model: state.model.clone(),
            color: self.color.clone(),
            status: state.status,
            tabs: state.tabs.iter().copied().collect(),
            created_ms: self.created_ms,
            access: self.launch.access.clone(),
            pending_permissions,
            title: state
                .title
                .clone()
                .or_else(|| state.auto_title.clone())
                .unwrap_or_else(|| self.provider_name.clone()),
            title_set: state.title.is_some(),
            archived: state.archived,
            updated_ms: state.updated_ms,
            queued: state.queued.len(),
        }
    }

    pub fn status(&self) -> Status {
        self.lock().status
    }

    /// The model chosen for this chat (`None` = the provider's default).
    pub fn model(&self) -> Option<String> {
        self.lock().model.clone()
    }

    /// Use `model` from the next turn on (saved). The backend is told by the caller.
    pub fn set_model(&self, model: Option<String>) {
        {
            let mut state = self.lock();
            state.model = model.filter(|m| !m.is_empty());
            state.updated_ms = now_ms();
        }
        self.save_meta();
        self.emit_update();
    }

    /// Rename the chat (`None` or blank: back to the automatic title). Saved.
    pub fn rename(&self, title: Option<String>) {
        {
            let mut state = self.lock();
            state.title = title
                .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|t| !t.is_empty());
            state.updated_ms = now_ms();
        }
        self.save_meta();
        self.emit_update();
    }

    pub fn is_archived(&self) -> bool {
        self.lock().archived
    }

    /// Archive (close) or reopen the chat. Saved.
    pub fn set_archived(&self, archived: bool) {
        {
            let mut state = self.lock();
            state.archived = archived;
            state.updated_ms = now_ms();
        }
        self.save_meta();
        self.emit_update();
    }

    /// Publish on `events` from now on (a session restored from disk starts with a bus of
    /// its own; the first command on it hands over the live one).
    pub fn attach_events(&self, events: &EventBus) {
        *self.events.lock().unwrap_or_else(|e| e.into_inner()) = events.clone();
    }

    /// Publish the whole [`SessionInfo`] as `ai.session` (with `session` and `status`, as the
    /// status events always had).
    pub fn emit_update(&self) {
        let data = serde_json::to_value(self.info()).unwrap_or_default();
        self.emit(topics::SESSION, data);
    }

    /// User messages so far.
    pub fn user_count(&self) -> usize {
        self.lock().users
    }

    /// Indexes of the user messages the agent has not taken yet.
    pub fn queued(&self) -> Vec<usize> {
        self.lock().queued.clone()
    }

    /// The agent took these queued user messages: `ai.message_taken` for each, and an update.
    /// Indexes that are not queued are ignored.
    pub fn take(&self, indexes: &[usize]) {
        let taken: Vec<usize> = {
            let mut state = self.lock();
            let taken: Vec<usize> = indexes
                .iter()
                .copied()
                .filter(|i| state.queued.contains(i))
                .collect();
            if taken.is_empty() {
                return;
            }
            state.queued.retain(|i| !taken.contains(i));
            for entry in state.transcript.iter_mut() {
                if entry["type"] == "user"
                    && entry["index"]
                        .as_u64()
                        .is_some_and(|i| taken.contains(&(i as usize)))
                    && let Some(map) = entry.as_object_mut()
                {
                    map.remove("queued");
                }
            }
            taken
        };
        for index in taken {
            self.emit(topics::MESSAGE_TAKEN, json!({ "index": index }));
        }
        self.emit_update();
    }

    /// The queued user messages, `(index, text)` in order.
    pub fn queued_messages(&self) -> Vec<(usize, String)> {
        let state = self.lock();
        state
            .queued
            .iter()
            .filter_map(|&index| {
                let entry =
                    state.transcript.iter().rev().find(|e| {
                        e["type"] == "user" && e["index"].as_u64() == Some(index as u64)
                    })?;
                Some((
                    index,
                    entry["text"].as_str().unwrap_or_default().to_string(),
                ))
            })
            .collect()
    }

    /// Take every queued message (the agent has them all now). Returns their indexes.
    pub fn take_all(&self) -> Vec<usize> {
        let queued = self.queued();
        self.take(&queued);
        queued
    }

    /// Forget the queued messages without taking them (the run that had them ended).
    pub fn drop_queued(&self) {
        let changed = {
            let mut state = self.lock();
            let changed = !state.queued.is_empty();
            state.queued.clear();
            for entry in state.transcript.iter_mut() {
                if let Some(map) = entry.as_object_mut() {
                    map.remove("queued");
                }
            }
            changed
        };
        if changed {
            self.emit_update();
        }
    }

    /// After a turn: idle, unless messages are still waiting (then the agent goes on).
    pub fn settle(&self) {
        let status = if self.is_cancelled() {
            Status::Stopped
        } else if self.queued().is_empty() {
            Status::Idle
        } else {
            Status::Running
        };
        self.set_status(status);
    }

    /// One-shot: where the next Claude Code run resumes its conversation (after an edit).
    pub fn take_resume_at(&self) -> Option<String> {
        self.lock().resume_at.take()
    }

    /// One-shot: how many Codex turns the next run rolls back (after an edit).
    pub fn take_rollback(&self) -> Option<u32> {
        self.lock().rollback.take()
    }

    /// Set what the next run does to continue after an edit (see [`Session::rewind`]).
    pub fn prepare_resume(&self, resume_at: Option<String>, rollback: Option<u32>) {
        let mut state = self.lock();
        state.resume_at = resume_at;
        state.rollback = rollback;
    }

    /// Forget the CLI's conversation: the next run starts a new one (saved).
    pub fn forget_conversation(&self) {
        self.lock().conversation = None;
        self.save_meta();
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
    pub fn restart(&self, events: EventBus) -> mpsc::UnboundedReceiver<Input> {
        *self.events.lock().unwrap_or_else(|e| e.into_inner()) = events;
        let (input, rx) = mpsc::unbounded_channel();
        {
            let mut run = self.run();
            run.input = input;
            run.cancel = CancellationToken::new();
            run.turn = CancellationToken::new();
            run.generation += 1;
        }
        // Messages still queued here were never taken by the old run, which ended by itself
        // (a stop, close or edit drops them first): they stay queued, and the message that
        // continues the chat takes them along (see `AiService::send`).
        self.set_status(Status::Idle);
        rx
    }

    /// API agents: a fresh token for the turn that starts now ([`Session::interrupt`]
    /// cancels it).
    pub fn begin_turn(&self) -> CancellationToken {
        let token = CancellationToken::new();
        self.run().turn = token.clone();
        token
    }

    /// Stop the current turn but keep the chat (and its backend) alive. `false` when there
    /// was nothing to interrupt.
    pub fn interrupt(&self) -> bool {
        if self.needs_restart() || self.status() != Status::Running {
            return false;
        }
        let run = self.run();
        run.turn.cancel();
        run.input.send(Input::Interrupt).is_ok()
    }

    /// Tell the running backend to use `model` from its next turn on.
    pub fn send_model(&self, model: Option<String>) {
        if !self.needs_restart() {
            let _ = self.run().input.send(Input::SetModel(model));
        }
    }

    /// Queue a user message; records and announces it. Never refused while the agent is
    /// busy: the message is then `queued` until the backend takes it ([`Session::take`]).
    /// Fails when the session has ended (`AiService::send` restarts it first). Returns the
    /// message's index.
    pub fn send(&self, text: String) -> Result<usize, String> {
        self.send_as(text.clone(), text)
    }

    /// Like [`Session::send`], but the backend gets `deliver` (the transcript shows `text`):
    /// after an edit, the messages the agent lost with the rewind go along.
    pub fn send_as(&self, text: String, deliver: String) -> Result<usize, String> {
        if self.needs_restart() {
            return Err(format!("session {} has ended", self.id));
        }
        let (index, queued, titled) = {
            let mut state = self.lock();
            let index = state.users;
            state.users += 1;
            let queued = state.status == Status::Running;
            if queued {
                state.queued.push(index);
            } else {
                // Busy from now on, in the same step: a second message right behind this
                // one is queued.
                state.status = Status::Running;
            }
            let titled = state.auto_title.is_none();
            if titled {
                state.auto_title = auto_title(&text);
            }
            (index, queued, titled && state.auto_title.is_some())
        };
        self.record_user(&text, index, queued);
        let mut event = json!({ "role": "user", "text": text, "index": index });
        if queued {
            event["queued"] = json!(true);
        }
        self.emit(topics::MESSAGE, event);
        if titled {
            self.save_meta();
        }
        let sent = self.run().input.send(Input::Message {
            text: deliver,
            index: Some(index),
            queued,
        });
        // An update either way: status, queued count, title.
        self.emit_update();
        sent.map_err(|_| format!("session {} is not accepting messages", self.id))?;
        Ok(index)
    }

    pub fn set_status(&self, status: Status) {
        let changed = {
            let mut state = self.lock();
            let changed = state.status != status;
            state.status = status;
            changed
        };
        if changed {
            self.emit_update();
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
                let mut entry = json!({ "type": "done", "reason": reason });
                {
                    let mut state = self.lock();
                    if let Some(at) = state.checkpoint.take() {
                        entry["at"] = json!(at);
                    }
                    if let Some(turn) = state.turn.take() {
                        entry["turn"] = json!(turn);
                    }
                }
                self.record(entry);
                // The status changes BEFORE `ai.done` goes out: a client that sends the next
                // message as soon as it sees `ai.done` must find the session idle, not busy
                // (still running when queued messages wait for the next turn).
                self.settle();
                self.emit(topics::DONE, json!({ "reason": reason }));
            }
            AgentEvent::Checkpoint(uuid) => self.lock().checkpoint = Some(uuid),
            AgentEvent::Turn(id) => self.lock().turn = Some(id),
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
        // Not for a deleted chat (see `save_meta`).
        if let Some(log) = self.log_path.as_ref().filter(|_| !self.is_closed())
            && let Err(err) = write_json(&with_suffix(log, "history.json"), &history)
        {
            tracing::warn!(%err, "cannot save the AI conversation");
        }
        *self.api_history.lock().unwrap_or_else(|e| e.into_inner()) = history;
    }

    // ------------------------------------------------------------ files

    /// What `<id>.meta.json` holds.
    pub fn saved(&self) -> SavedSession {
        let state = self.lock();
        SavedSession {
            id: self.id.clone(),
            provider: self.provider.clone(),
            model: state.model.clone(),
            color: self.color.clone(),
            created_ms: self.created_ms,
            launch: self.launch.clone(),
            conversation: state.conversation.clone(),
            title: state
                .title
                .clone()
                .or_else(|| state.auto_title.clone())
                .unwrap_or_default(),
            title_set: state.title.is_some(),
            archived: state.archived,
            updated_ms: state.updated_ms,
        }
    }

    /// `ai.session_edit`: cut the transcript (memory and file) just before user message
    /// `index` and say what the conversation looked like before it (see [`Rewind`]). `None`
    /// when there is no such message. The backend must have ended.
    pub fn rewind(&self, index: usize) -> Option<Rewind> {
        let (rewind, kept) = {
            let mut state = self.lock();
            let entries = &state.transcript;
            let user_positions: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e["type"] == "user")
                .map(|(i, _)| i)
                .collect();
            let cut = *user_positions.get(index)?;
            let before = &entries[..cut];
            let text_of = |e: &Value| e["text"].as_str().unwrap_or_default().to_string();
            let last_done = before.iter().rposition(|e| e["type"] == "done");
            let last_checkpoint = before
                .iter()
                .rposition(|e| e["type"] == "done" && e["at"].is_string());
            // Claude Code goes back to the last checkpoint, Codex rolls back the turns after
            // the last finished turn; both lose the user messages after that point.
            let boundary = last_checkpoint.or(last_done);
            let start = boundary.map_or(0, |b| b + 1);
            let rewind = Rewind {
                user_texts: before
                    .iter()
                    .filter(|e| e["type"] == "user")
                    .map(text_of)
                    .collect(),
                prefix: before[start..]
                    .iter()
                    .filter(|e| e["type"] == "user")
                    .map(text_of)
                    .collect(),
                resume_at: last_checkpoint
                    .and_then(|b| before[b]["at"].as_str().map(str::to_string)),
                turns_before: last_done.is_some(),
                // Only turns Codex really has: a `done` without `turn` (a `turn/start` that
                // failed, a stop before the thread started) has nothing to roll back.
                turns_after: entries[start..]
                    .iter()
                    .filter(|e| e["type"] == "done" && e["turn"].is_string())
                    .count() as u32,
                earlier: before
                    .iter()
                    .filter(|e| matches!(e["type"].as_str(), Some("user" | "assistant")))
                    .map(|e| {
                        (
                            e["type"].as_str().unwrap_or_default().to_string(),
                            text_of(e),
                        )
                    })
                    .collect(),
            };
            state.transcript.truncate(cut);
            state.users = index;
            state.queued.clear();
            state.checkpoint = None;
            state.turn = None;
            if index == 0 {
                state.auto_title = None;
            }
            state.updated_ms = now_ms();
            (rewind, state.transcript.clone())
        };
        if let Some(path) = &self.log_path {
            let mut text = String::new();
            for entry in &kept {
                let mut entry = entry.clone();
                if let Some(map) = entry.as_object_mut() {
                    map.remove("queued");
                }
                text.push_str(&format!("{entry}\n"));
            }
            if let Err(err) = std::fs::write(path, text) {
                tracing::warn!(%err, path = %path.display(), "cannot rewrite AI transcript");
            }
        }
        Some(rewind)
    }

    /// Write `<id>.meta.json` (the session is listed again after a browser restart). Not for
    /// a deleted chat: a late write (a close or rename racing the delete, the last events of
    /// its run) would bring it back after a restart.
    pub fn save_meta(&self) {
        if self.is_closed() {
            return;
        }
        if let Some(log) = &self.log_path
            && let Err(err) = write_json(&meta_path(log), &self.saved())
        {
            tracing::warn!(%err, "cannot save the AI session");
        }
    }

    /// Delete every file of the session (`ai.session_delete`): meta, transcript, history.
    pub fn remove_files(&self) {
        let Some(log) = &self.log_path else { return };
        let _ = std::fs::remove_file(meta_path(log));
        let _ = std::fs::remove_file(log);
        let _ = std::fs::remove_file(with_suffix(log, "history.json"));
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

    /// A user message in the transcript; `queued` only in memory (it ends with the run).
    fn record_user(&self, text: &str, index: usize, queued: bool) {
        self.record(json!({ "type": "user", "text": text, "index": index }));
        if queued {
            let mut state = self.lock();
            if let Some(entry) = state
                .transcript
                .iter_mut()
                .rev()
                .find(|e| e["type"] == "user" && e["index"] == index)
            {
                entry["queued"] = json!(true);
            }
        }
    }

    /// Append to the transcript (memory + jsonl file).
    fn record(&self, mut entry: Value) {
        let now = now_ms();
        entry["ts_ms"] = json!(now);
        self.lock().updated_ms = now;
        // Not for a deleted chat (see `save_meta`).
        if let Some(path) = self.log_path.as_ref().filter(|_| !self.is_closed()) {
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

/// API agents, `ai.session_edit`: the model conversation up to just before user message
/// `index`. `user_texts` are the chat's user messages before it, in order (the transcript's).
/// A history message may hold several of them (sent together, joined with a blank line):
/// those before `index` stay. `None` when the history does not match the transcript.
pub fn rewind_history(
    history: &[Message],
    user_texts: &[String],
    index: usize,
) -> Option<Vec<Message>> {
    let mut next = 0; // next transcript message to find
    for (at, message) in history.iter().enumerate() {
        let Message::User { text } = message else {
            continue;
        };
        let mut rest = text.as_str();
        let mut held: Vec<&str> = Vec::new();
        loop {
            if next == index {
                let mut kept = history[..at].to_vec();
                if !held.is_empty() {
                    kept.push(Message::User {
                        text: held.join("\n\n"),
                    });
                }
                return Some(kept);
            }
            let expected = user_texts.get(next)?;
            match rest.strip_prefix(expected.as_str()) {
                Some(after) => {
                    held.push(expected);
                    next += 1;
                    rest = after.strip_prefix("\n\n").unwrap_or(after);
                    if rest.is_empty() {
                        break;
                    }
                }
                None if held.is_empty() => return None,
                None => break,
            }
        }
    }
    // The edited message was not sent to the model yet (stopped before it): all of it stays.
    (next == index).then(|| history.to_vec())
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
        assert_eq!(
            rx.recv().await.unwrap(),
            Input::Message {
                text: "hi".into(),
                index: Some(0),
                queued: false
            }
        );
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
        assert!(matches!(rx.recv().await.unwrap(), Input::Message { text, .. } if text == "again"));
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
        assert_eq!(restored.model().as_deref(), Some("sonnet"));
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

        // The title came from the first message; the user entry got its index.
        assert_eq!(restored.info().title, "hello");
        assert_eq!(restored.transcript()[0]["index"], 0);
        assert_eq!(restored.user_count(), 1);

        restored.remove_files();
        assert!(saved_sessions(dir.path()).is_empty());
        assert!(!log.exists());
        assert!(!log.with_extension("history.json").exists());
    }

    #[test]
    fn titles_are_cut_from_the_first_message() {
        assert_eq!(
            auto_title("  hello \n\t world  ").as_deref(),
            Some("hello world")
        );
        assert_eq!(auto_title(" \n "), None);
        let long = "word ".repeat(30);
        let title = auto_title(&long).unwrap();
        assert_eq!(title.chars().count(), TITLE_MAX);
        assert!(title.ends_with('…') && !title.contains("  "), "{title}");
        let exact = "x".repeat(TITLE_MAX);
        assert_eq!(auto_title(&exact).unwrap(), exact);
        let cyrillic = "привіт ".repeat(20);
        assert!(auto_title(&cyrillic).unwrap().chars().count() <= TITLE_MAX);
    }

    fn new_session(dir: &Path, id: &str) -> (Session, mpsc::UnboundedReceiver<Input>) {
        let (mut session, rx) = Session::new(
            id.into(),
            "claude-code".into(),
            None,
            COLORS[0].into(),
            EventBus::new(),
            Some(dir.join(format!("{id}.jsonl"))),
        );
        session.provider_name = "Claude Code".into();
        (session, rx)
    }

    #[tokio::test]
    async fn title_rename_archive_and_model_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (session, _rx) = new_session(dir.path(), "claude-code-1");
        session.save_meta();
        // Until the first message: the provider's name, never the id.
        assert_eq!(session.info().title, "Claude Code");
        assert!(!session.info().title_set);
        session
            .send("Find   the cheapest\nlaptop with 32 GB of RAM and a good keyboard".into())
            .unwrap();
        let info = session.info();
        assert_eq!(
            info.title,
            "Find the cheapest laptop with 32 GB of RAM and…"
        );
        // The automatic title is set once.
        session.apply(AgentEvent::Done("end_turn".into()));
        session.send("second".into()).unwrap();
        assert!(session.info().title.starts_with("Find the cheapest"));

        session.rename(Some("  Laptops  ".into()));
        assert_eq!(
            (session.info().title.as_str(), session.info().title_set),
            ("Laptops", true)
        );
        session.set_model(Some("opus".into()));
        session.set_archived(true);
        let restore = || {
            let (meta, path) = saved_sessions(dir.path()).into_iter().next().unwrap();
            Session::restore(meta, path)
        };
        let restored = restore();
        let info = restored.info();
        assert_eq!(info.title, "Laptops");
        assert!(info.title_set && info.archived);
        assert_eq!(info.model.as_deref(), Some("opus"));
        assert!(info.updated_ms >= info.created_ms);
        // Renamed to "": back to the automatic title.
        restored.rename(Some(" ".into()));
        restored.set_archived(false);
        let info = restore().info();
        assert_eq!((info.title_set, info.archived), (false, false));
        assert!(info.title.starts_with("Find the cheapest"));
    }

    #[test]
    fn old_meta_files_get_defaults_and_a_title() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("codex-3.meta.json"),
            r##"{"id":"codex-3","provider":"codex","model":null,"color":"#3b82f6","created_ms":5,
                "launch":{"toolsets":["core"],"max_steps":30,"access":null}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("codex-3.jsonl"),
            "{\"type\":\"user\",\"text\":\"open  example.com\",\"ts_ms\":10}\n\
             {\"type\":\"assistant\",\"text\":\"ok\",\"ts_ms\":20}\n\
             {\"type\":\"user\",\"text\":\"thanks\",\"ts_ms\":30}\n",
        )
        .unwrap();
        let (meta, path) = saved_sessions(dir.path()).into_iter().next().unwrap();
        assert_eq!(
            (meta.title.as_str(), meta.archived, meta.updated_ms),
            ("", false, 0)
        );
        let session = Session::restore(meta, path);
        let info = session.info();
        assert_eq!(info.title, "open example.com");
        assert_eq!(info.updated_ms, 30);
        assert_eq!(info.queued, 0);
        let indexes: Vec<_> = session
            .transcript()
            .iter()
            .filter(|e| e["type"] == "user")
            .map(|e| e["index"].clone())
            .collect();
        assert_eq!(indexes, [json!(0), json!(1)]);
    }

    #[tokio::test]
    async fn messages_sent_while_busy_are_queued_until_taken() {
        let bus = EventBus::new();
        let mut events = bus.subscribe();
        let (session, mut rx) = Session::new(
            "custom-1".into(),
            "custom".into(),
            None,
            COLORS[0].into(),
            bus,
            None,
        );
        assert_eq!(session.send("first".into()).unwrap(), 0);
        assert_eq!(session.status(), Status::Running);
        assert_eq!(session.send("more".into()).unwrap(), 1);
        assert_eq!(session.queued(), [1]);
        assert_eq!(session.info().queued, 1);
        assert!(matches!(
            rx.recv().await,
            Some(Input::Message { queued: false, .. })
        ));
        assert!(matches!(
            rx.recv().await,
            Some(Input::Message {
                queued: true,
                index: Some(1),
                ..
            })
        ));
        let user: Vec<Value> = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|e| e.topic == "ai.message")
            .map(|e| e.data)
            .collect();
        assert_eq!(user[0]["queued"], Value::Null);
        assert_eq!(
            (user[1]["queued"].clone(), user[1]["index"].clone()),
            (json!(true), json!(1))
        );
        assert_eq!(session.transcript()[1]["queued"], true);

        // A turn ends with a message still waiting: the agent goes on (running).
        session.apply(AgentEvent::Done("end_turn".into()));
        assert_eq!(session.status(), Status::Running);
        session.take(&[1]);
        let taken: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        let event = taken
            .iter()
            .find(|e| e.topic == "ai.message_taken")
            .unwrap();
        assert_eq!(event.data["index"], 1);
        let update = taken.iter().rfind(|e| e.topic == "ai.session").unwrap();
        assert_eq!(update.data["queued"], 0);
        assert_eq!(update.data["title"], "first");
        assert!(session.transcript()[1].get("queued").is_none());
        session.apply(AgentEvent::Done("end_turn".into()));
        assert_eq!(session.status(), Status::Idle);
    }

    #[tokio::test]
    async fn interrupt_cancels_the_turn_and_tells_the_backend() {
        let (session, mut rx) = Session::new(
            "custom-1".into(),
            "custom".into(),
            None,
            COLORS[0].into(),
            EventBus::new(),
            None,
        );
        assert!(!session.interrupt(), "nothing runs");
        session.send("go".into()).unwrap();
        rx.recv().await.unwrap();
        let turn = session.begin_turn();
        assert!(session.interrupt());
        assert!(turn.is_cancelled() && !session.is_cancelled());
        assert_eq!(rx.recv().await, Some(Input::Interrupt));
        session.send_model(Some("m2".into()));
        assert_eq!(rx.recv().await, Some(Input::SetModel(Some("m2".into()))));
    }

    #[tokio::test]
    async fn rewind_cuts_the_transcript_and_finds_the_resume_point() {
        let dir = tempfile::tempdir().unwrap();
        let (session, _rx) = new_session(dir.path(), "claude-code-2");
        let log = dir.path().join("claude-code-2.jsonl");
        session.send("one".into()).unwrap();
        session.apply(AgentEvent::Text("a1".into()));
        session.apply(AgentEvent::Checkpoint("uuid-1".into()));
        session.apply(AgentEvent::Done("end_turn".into()));
        session.send("two".into()).unwrap();
        session.send("two-b".into()).unwrap(); // queued: folded into the same turn
        session.apply(AgentEvent::Text("a2".into()));
        session.apply(AgentEvent::Checkpoint("uuid-2".into()));
        session.apply(AgentEvent::Done("end_turn".into()));
        assert_eq!(session.transcript()[2]["at"], "uuid-1");

        // Editing "two-b": back to the end of turn 1; "two" is lost with it and goes again.
        let rewind = session.rewind(2).unwrap();
        assert_eq!(rewind.resume_at.as_deref(), Some("uuid-1"));
        assert_eq!(rewind.prefix, ["two"]);
        assert_eq!(rewind.user_texts, ["one", "two"]);
        assert_eq!(
            rewind.turns_after, 0,
            "no Codex turns in a Claude Code chat"
        );
        assert!(rewind.turns_before);
        assert_eq!(session.user_count(), 2);
        assert!(session.queued().is_empty());
        let kept: Vec<String> = session
            .transcript()
            .iter()
            .map(|e| e["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kept, ["user", "assistant", "done", "user"]);
        // The file is cut too.
        assert_eq!(read_transcript(&log).len(), 4);
        // The edited message gets the index it replaces.
        session.cancel();
        let _rx = session.restart(EventBus::new());
        assert_eq!(session.send("two-c".into()).unwrap(), 2);

        // Editing the first message: from the start, and a new automatic title.
        let rewind = session.rewind(0).unwrap();
        assert_eq!((rewind.resume_at, rewind.turns_before), (None, false));
        assert!(rewind.prefix.is_empty());
        assert!(session.transcript().is_empty());
        assert_eq!(session.info().title, "Claude Code");
        assert!(session.rewind(5).is_none());
    }

    /// Codex: only `done`s that ended a turn of the thread count for `thread/rollback`; a
    /// `turn/start` that failed (or a stop before the thread started) left none.
    #[tokio::test]
    async fn codex_rollback_counts_only_turns_codex_has() {
        let (session, _rx) = Session::new(
            "codex-1".into(),
            "codex".into(),
            None,
            COLORS[0].into(),
            EventBus::new(),
            None,
        );
        session.send("one".into()).unwrap();
        session.apply(AgentEvent::Turn("u1".into()));
        session.apply(AgentEvent::Done("end_turn".into()));
        session.send("two".into()).unwrap();
        session.apply(AgentEvent::Done("error: turn/start: overloaded".into()));
        session.send("three".into()).unwrap();
        session.apply(AgentEvent::Turn("u2".into()));
        session.apply(AgentEvent::Done("end_turn".into()));
        assert_eq!(session.transcript()[1]["turn"], "u1");
        assert!(session.transcript()[3].get("turn").is_none());
        // Editing "one": both real turns go, not the failed start.
        let rewind = session.rewind(0).unwrap();
        assert_eq!(rewind.turns_after, 2);
    }

    /// A deleted chat writes no meta file any more (a close or rename racing the delete
    /// would bring it back after a restart).
    #[tokio::test]
    async fn a_deleted_chat_saves_no_meta() {
        let dir = tempfile::tempdir().unwrap();
        let (session, _rx) = new_session(dir.path(), "claude-code-5");
        session.save_meta();
        assert_eq!(saved_sessions(dir.path()).len(), 1);
        session.mark_closed();
        session.remove_files();
        session.set_archived(true);
        session.rename(Some("late".into()));
        session.set_model(Some("opus".into()));
        // The last events of its run and its history neither.
        session.apply(AgentEvent::Text("late".into()));
        session.set_api_history(vec![Message::User { text: "x".into() }]);
        assert!(saved_sessions(dir.path()).is_empty());
        for file in ["meta.json", "jsonl", "history.json"] {
            let path = dir.path().join(format!("claude-code-5.{file}"));
            assert!(!path.exists(), "{}", path.display());
        }
    }

    /// Messages a run never took stay queued when it ended by itself: the next run gets them
    /// (`AiService::send`), so a restart does not drop them.
    #[tokio::test]
    async fn queued_messages_outlive_a_run_that_ended_by_itself() {
        let (session, _rx) = Session::new(
            "codex-2".into(),
            "codex".into(),
            None,
            COLORS[0].into(),
            EventBus::new(),
            None,
        );
        session.send("first".into()).unwrap();
        session.send("second".into()).unwrap();
        session.send("third".into()).unwrap();
        session.take(&[1]);
        assert_eq!(session.queued(), [2]);
        // The CLI exits by itself.
        session.set_status(Status::Failed);
        let _rx = session.restart(EventBus::new());
        assert_eq!(session.status(), Status::Idle);
        assert_eq!(session.queued_messages(), [(2, "third".to_string())]);
        assert_eq!(session.info().queued, 1);
        assert_eq!(session.transcript()[2]["queued"], true);
    }

    #[test]
    fn api_history_rewinds_to_before_the_edited_message() {
        let user = |t: &str| Message::User { text: t.into() };
        let assistant = |t: &str| Message::Assistant {
            text: t.into(),
            calls: vec![],
            raw: None,
        };
        let tool = Message::Tool {
            call_id: "c".into(),
            name: "x".into(),
            output: "{}".into(),
            is_error: false,
        };
        let history = vec![
            user("one"),
            assistant("a1"),
            user("two\n\nthree"), // sent while busy: joined
            assistant(""),
            tool.clone(),
            user("four"), // injected after a tool result
            assistant("a4"),
        ];
        let texts: Vec<String> = ["one", "two", "three", "four"].map(String::from).to_vec();
        assert_eq!(rewind_history(&history, &texts, 0).unwrap(), vec![]);
        assert_eq!(
            rewind_history(&history, &texts, 1).unwrap(),
            vec![user("one"), assistant("a1")]
        );
        assert_eq!(
            rewind_history(&history, &texts, 2).unwrap(),
            vec![user("one"), assistant("a1"), user("two")]
        );
        assert_eq!(
            rewind_history(&history, &texts, 3).unwrap(),
            history[..5].to_vec()
        );
        // Not in the history yet (stopped before the model saw it): everything stays.
        let mut five = texts.clone();
        five.push("five".into());
        assert_eq!(rewind_history(&history, &five, 4).unwrap(), history);
        // A history that does not match the transcript.
        assert!(rewind_history(&[user("other")], &texts, 1).is_none());
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
