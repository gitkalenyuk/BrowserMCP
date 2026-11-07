//! Claude Code as an agent: `claude -p --input-format stream-json --output-format stream-json`.
//!
//! * Input (stdin), one JSON object per line:
//!   `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"…"}]}}`.
//!   The process stays alive between messages until stdin closes or it is killed.
//! * Output (stdout), one JSON object per line: `system` (`init` lists MCP servers and gives the
//!   `session_id`), `stream_event` (partial text, with `--include-partial-messages`),
//!   `assistant` (text and `tool_use` blocks), `user` (`tool_result` blocks), `result` (end of
//!   one answer).
//! * Tools: only the browser's MCP server (`--mcp-config <file> --strict-mcp-config`), always
//!   allowed (`--allowedTools mcp__browsermcp`). Other tools follow the session's
//!   [`PermissionMode`]: with `--permission-prompt-tool stdio` the CLI asks US before a tool
//!   call, over the same pipes (the Claude Agent SDK control protocol):
//!
//!   ```text
//!   ← {"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool",
//!        "tool_name":"Bash","input":{…},"tool_use_id":"toolu_…","title":"…",…}}
//!   → {"type":"control_response","response":{"subtype":"success","request_id":"r1",
//!        "response":{"behavior":"allow","updatedInput":{…}}}}           (or)
//!        "response":{"behavior":"deny","message":"The user denied this action: …"}}}
//!   ← {"type":"control_cancel_request","request_id":"r1"}   (the CLI gave up waiting)
//!   ```
//!
//!   We send `initialize` first, as the SDK does. "Browser tools only" keeps the old
//!   behaviour: `--permission-mode dontAsk`, no prompts, anything else is refused.
//! * Continuing: the `session_id` of `system/init` is kept; the next run of a stopped session
//!   passes `--resume=<id>` (same working folder: Claude Code stores conversations per folder).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::mpsc;

use super::access::{Access, PermissionMode};
use super::permission::{Ask, Decision, NewPermission, PermissionKind};
use super::{StderrTail, command, join_reader, kill_tree, spawn};
use crate::session::{AgentEvent, Session, Status, summarize};

/// Name of the MCP server in the generated config; tools appear as `mcp__browsermcp__<tool>`.
pub const SERVER: &str = "browsermcp";
const TOOL_PREFIX: &str = "mcp__browsermcp__";

pub struct ClaudeCode {
    pub session: Arc<Session>,
    pub binary: PathBuf,
    pub model: Option<String>,
    /// MCP URL including `?agent=<id>&toolsets=…`.
    pub mcp_url: String,
    /// Where the MCP config file is written (`data/ai/sessions/<id>.mcp.json`).
    pub mcp_config: PathBuf,
    /// Permission mode and folders (`access.cwd` is the working directory).
    pub access: Access,
    /// Claude Code session id to continue (`--resume`).
    pub resume: Option<String>,
}

/// The MCP config file content.
pub fn mcp_config(url: &str) -> Value {
    json!({ "mcpServers": { SERVER: { "type": "http", "url": url } } })
}

/// What the command line depends on.
pub struct Args<'a> {
    pub mcp_config: &'a str,
    pub model: Option<&'a str>,
    pub append_prompt: &'a str,
    pub access: &'a Access,
    pub resume: Option<&'a str>,
}

/// Command-line arguments (without the binary).
pub fn args(a: &Args) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--strict-mcp-config",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mode = a.access.permission_mode;
    if let Some(cli_mode) = mode.cli_mode() {
        args.extend(["--permission-mode".into(), cli_mode.into()]);
    }
    if mode == PermissionMode::FullAccess {
        // Required next to `bypassPermissions` by current Claude Code versions.
        args.push("--allow-dangerously-skip-permissions".into());
    }
    if mode.prompts() {
        args.extend(["--permission-prompt-tool".into(), "stdio".into()]);
    }
    // Options taking several values (`--mcp-config`, `--allowedTools`, `--add-dir`) are each
    // followed by another flag, never by a free value.
    args.extend(["--mcp-config".into(), a.mcp_config.into()]);
    args.extend(["--allowedTools".into(), format!("mcp__{SERVER}")]);
    for dir in &a.access.add_dirs {
        args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
    }
    args.extend(["--append-system-prompt".into(), a.append_prompt.into()]);
    if let Some(model) = a.model.filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(id) = a.resume.filter(|r| !r.is_empty()) {
        // `--flag=value`, like the SDK: the id can never be taken for another option.
        args.push(format!("--resume={id}"));
    }
    args
}

/// One user message as a stream-json input line (with the trailing newline).
pub fn user_line(text: &str) -> String {
    let line = json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
    });
    format!("{line}\n")
}

/// The `initialize` control request (sent once, before the first message).
pub fn initialize_line() -> String {
    let line = json!({
        "type": "control_request",
        "request_id": "bmcp-init",
        "request": { "subtype": "initialize", "hooks": null },
    });
    format!("{line}\n")
}

/// A control message from the CLI.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// `can_use_tool`: may the agent run this tool call?
    CanUseTool {
        request_id: String,
        /// The CLI's tool name (`Bash`, `mcp__x__y`), used in "always allow" rules.
        tool_name: String,
        input: Value,
        permission: Box<NewPermission>,
    },
    /// The CLI no longer waits for an answer to `request_id`.
    Cancel(String),
    /// A request we do not handle (hooks, SDK MCP servers): answered with an error.
    Unsupported { request_id: String, subtype: String },
    /// The answer to our own `initialize`.
    Response,
}

/// Recognise control-protocol lines; `None` for ordinary output.
pub fn control(v: &Value) -> Option<Control> {
    match v["type"].as_str()? {
        "control_request" => {
            let request_id = v["request_id"].as_str().unwrap_or_default().to_string();
            let request = &v["request"];
            let subtype = request["subtype"].as_str().unwrap_or_default();
            if subtype != "can_use_tool" {
                return Some(Control::Unsupported {
                    request_id,
                    subtype: subtype.to_string(),
                });
            }
            let raw_name = request["tool_name"].as_str().unwrap_or("tool").to_string();
            let text = |key: &str| {
                request[key]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            // `decision_reason` is a string in some versions, `{reason, …}` in others.
            let reason = text("decision_reason")
                .or_else(|| {
                    request["decision_reason"]["reason"]
                        .as_str()
                        .map(str::to_string)
                })
                .or_else(|| text("description"))
                .or_else(|| {
                    text("blocked_path").map(|p| format!("Outside the allowed folders: {p}"))
                });
            let permission = NewPermission {
                tool: tool_name(&raw_name),
                kind: PermissionKind::Tool,
                title: text("title").or_else(|| text("display_name")),
                reason,
                input: request["input"].clone(),
            };
            Some(Control::CanUseTool {
                request_id,
                tool_name: raw_name,
                input: request["input"].clone(),
                permission: Box::new(permission),
            })
        }
        "control_cancel_request" => Some(Control::Cancel(
            v["request_id"].as_str().unwrap_or_default().to_string(),
        )),
        "control_response" => Some(Control::Response),
        _ => None,
    }
}

/// The `control_response` line for a permission decision.
pub fn permission_response(
    request_id: &str,
    tool_name: &str,
    input: &Value,
    decision: &Decision,
) -> String {
    let response = match decision {
        Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
        Decision::AllowAlways => json!({
            "behavior": "allow",
            "updatedInput": input,
            // Also tell the CLI, so it stops asking about this tool in this session.
            "updatedPermissions": [{
                "type": "addRules",
                "rules": [{ "toolName": tool_name }],
                "behavior": "allow",
                "destination": "session",
            }],
        }),
        Decision::Deny { .. } | Decision::Cancelled => {
            json!({ "behavior": "deny", "message": decision.deny_message() })
        }
    };
    let line = json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    });
    format!("{line}\n")
}

/// The `control_response` line refusing a request we do not support.
pub fn error_response(request_id: &str, error: &str) -> String {
    let line = json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": error },
    });
    format!("{line}\n")
}

/// Turns stream-json output lines into [`AgentEvent`]s.
#[derive(Debug, Default)]
pub struct Parser {
    /// tool_use id → tool name, to name the matching tool_result.
    tools: HashMap<String, String>,
}

impl Parser {
    pub fn line(&mut self, line: &str) -> Vec<AgentEvent> {
        match serde_json::from_str::<Value>(line) {
            Ok(v) => self.value(&v),
            Err(_) => Vec::new(), // not JSON (should not happen); ignore
        }
    }

    pub fn value(&mut self, v: &Value) -> Vec<AgentEvent> {
        match v["type"].as_str().unwrap_or_default() {
            "system" if v["subtype"] == "init" => {
                let mut events = init_notices(v);
                if let Some(id) = v["session_id"].as_str().filter(|s| !s.is_empty()) {
                    events.push(AgentEvent::ConversationId(id.to_string()));
                }
                events
            }
            "stream_event" => {
                let event = &v["event"];
                let delta = &event["delta"];
                if event["type"] == "content_block_delta"
                    && delta["type"] == "text_delta"
                    && let Some(text) = delta["text"].as_str()
                {
                    return vec![AgentEvent::TextDelta(text.to_string())];
                }
                Vec::new()
            }
            "assistant" => self.assistant(&v["message"]["content"]),
            "user" => self.tool_results(&v["message"]["content"]),
            "result" => {
                let reason = if v["is_error"] == true {
                    let detail = v["result"].as_str().unwrap_or_default();
                    format!(
                        "error: {} {detail}",
                        v["subtype"].as_str().unwrap_or("error")
                    )
                    .trim()
                    .to_string()
                } else {
                    "end_turn".to_string()
                };
                vec![AgentEvent::Done(reason)]
            }
            _ => Vec::new(),
        }
    }

    fn assistant(&mut self, content: &Value) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for block in content.as_array().into_iter().flatten() {
            match block["type"].as_str().unwrap_or_default() {
                "text" => {
                    let text = block["text"].as_str().unwrap_or_default();
                    if !text.is_empty() {
                        events.push(AgentEvent::Text(text.to_string()));
                    }
                }
                "tool_use" => {
                    let name = tool_name(block["name"].as_str().unwrap_or_default());
                    if let Some(id) = block["id"].as_str() {
                        self.tools.insert(id.to_string(), name.clone());
                    }
                    events.push(AgentEvent::ToolCall {
                        name,
                        args: block["input"].clone(),
                    });
                }
                _ => {} // thinking etc.
            }
        }
        events
    }

    fn tool_results(&mut self, content: &Value) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for block in content.as_array().into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let id = block["tool_use_id"].as_str().unwrap_or_default();
            let name = self.tools.remove(id).unwrap_or_else(|| "tool".into());
            let text = result_text(&block["content"]);
            let output = serde_json::from_str::<Value>(&text).ok();
            events.push(AgentEvent::ToolResult {
                name,
                ok: block["is_error"] != true,
                summary: summarize(&Value::String(text)),
                output,
            });
        }
        events
    }
}

/// `mcp__browsermcp__tabs_open` → `tabs_open`; other tools keep their name (`Bash`).
fn tool_name(name: &str) -> String {
    name.strip_prefix(TOOL_PREFIX).unwrap_or(name).to_string()
}

/// tool_result content: a string, or blocks of which we keep the text ones.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Warn when the browser's MCP server did not connect (the agent would have no tools).
fn init_notices(v: &Value) -> Vec<AgentEvent> {
    v["mcp_servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["name"] == SERVER && s["status"] != "connected")
        .map(|s| {
            AgentEvent::Notice(format!(
                "Claude Code could not connect to the BrowserMCP MCP server (status: {})",
                s["status"].as_str().unwrap_or("unknown")
            ))
        })
        .collect()
}

/// The child's stdin, shared by the message loop and the permission answers. `None` once
/// closed (stopping closes it first: claude exits on EOF).
type Stdin = Arc<tokio::sync::Mutex<Option<ChildStdin>>>;

async fn write_line(stdin: &Stdin, line: &str) -> std::io::Result<()> {
    let mut guard = stdin.lock().await;
    let Some(stdin) = guard.as_mut() else {
        return Err(std::io::Error::other("stdin is closed"));
    };
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await
}

/// Writes user messages to the CLI.
struct Writer {
    stdin: Stdin,
    unanswered: Arc<Mutex<Vec<String>>>,
    /// `initialize` was sent (it goes out once, right before the first message).
    initialized: bool,
}

impl Writer {
    async fn message(&mut self, session: &Session, text: &str) {
        lock(&self.unanswered).push(text.to_string());
        let mut written = Ok(());
        if !self.initialized {
            self.initialized = true;
            written = write_line(&self.stdin, &initialize_line()).await;
        }
        if written.is_ok() {
            written = write_line(&self.stdin, &user_line(text)).await;
        }
        if let Err(err) = written {
            session.apply(AgentEvent::Notice(format!(
                "cannot write to Claude Code: {err}"
            )));
        }
    }
}

/// How one CLI process ended.
enum Exit {
    /// Stopped by the user.
    Stopped,
    /// The process ended by itself.
    Exited {
        status: std::io::Result<std::process::ExitStatus>,
        stderr: String,
        /// It printed `system/init`: it got as far as loading the conversation.
        started: bool,
        /// Messages written that it never answered (to replay on a fresh conversation).
        unanswered: Vec<String>,
    },
    /// Could not start at all.
    Failed(String),
}

impl ClaudeCode {
    /// The session task: run the CLI until stopped or it exits. When resuming fails before
    /// the conversation loads (e.g. Claude Code no longer has it), starts a fresh conversation
    /// in the same chat, says so, and sends the unanswered messages again.
    pub async fn run(self, mut input: mpsc::UnboundedReceiver<String>) {
        let session = self.session.clone();
        let fail = |message: String| {
            session.apply(AgentEvent::Notice(message.clone()));
            session.apply(AgentEvent::Done(format!("error: {message}")));
            session.set_status(Status::Failed);
        };

        let config = mcp_config(&self.mcp_url).to_string();
        if let Err(err) = std::fs::write(&self.mcp_config, config) {
            session.cancel_permissions();
            return fail(format!("cannot write {}: {err}", self.mcp_config.display()));
        }
        let mut resume = self.resume.clone();
        let mut replay = Vec::new();
        loop {
            let exit = self
                .run_process(resume.as_deref(), &mut input, replay)
                .await;
            session.cancel_permissions();
            match exit {
                Exit::Stopped => {
                    session.set_status(Status::Stopped);
                    break;
                }
                Exit::Failed(message) => {
                    fail(message);
                    break;
                }
                Exit::Exited {
                    started: false,
                    unanswered,
                    stderr,
                    ..
                } if resume.is_some() && lost_conversation(&stderr) => {
                    tracing::warn!(%stderr, "claude --resume failed; starting a new conversation");
                    session.apply(AgentEvent::Notice(
                        "Claude Code could not continue its earlier conversation, so this chat goes on in a new one: the agent does not see the messages above.".into(),
                    ));
                    resume = None;
                    replay = unanswered;
                }
                Exit::Exited { status, stderr, .. } => {
                    if status.as_ref().is_ok_and(|s| s.success()) {
                        if session.status() == Status::Running {
                            session.apply(AgentEvent::Done("stopped".into()));
                        }
                        session.set_status(Status::Stopped);
                    } else {
                        let code = status
                            .map(|s| s.to_string())
                            .unwrap_or_else(|e| e.to_string());
                        fail(format!("Claude Code exited ({code}): {stderr}"));
                    }
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(&self.mcp_config);
    }

    /// One `claude` process: relay messages and permission answers until it ends.
    async fn run_process(
        &self,
        resume: Option<&str>,
        input: &mut mpsc::UnboundedReceiver<String>,
        replay: Vec<String>,
    ) -> Exit {
        let session = self.session.clone();
        let cancel = session.cancel_token();
        let mut cmd = command(&self.binary, &self.access.cwd);
        cmd.args(args(&Args {
            mcp_config: &self.mcp_config.to_string_lossy(),
            model: self.model.as_deref(),
            append_prompt: &crate::agent::system_prompt(&session.id),
            access: &self.access,
            resume,
        }));
        // `tree` also covers the real agent behind an npm `claude.cmd` shim (a grandchild).
        let (mut child, tree) = match spawn(&mut cmd) {
            Ok(spawned) => spawned,
            Err(err) => {
                return Exit::Failed(format!("cannot start {}: {err}", self.binary.display()));
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Exit::Failed("child process has no stdio".into());
        };
        let stdin: Stdin = Arc::new(tokio::sync::Mutex::new(Some(stdin)));
        let stderr = StderrTail::collect(stderr, "claude");
        // Messages written but not answered yet (a `result` answers the oldest).
        let unanswered: Arc<Mutex<Vec<String>>> = Arc::default();
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // stdout → events and permission requests.
        let reader = {
            let (session, stdin, unanswered, started) = (
                session.clone(),
                stdin.clone(),
                unanswered.clone(),
                started.clone(),
            );
            tokio::spawn(async move {
                let mut parser = Parser::default();
                // CLI request id → our permission id, for `control_cancel_request`.
                let waiting: Arc<Mutex<HashMap<String, String>>> = Arc::default();
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if let Some(control) = control(&v) {
                        handle_control(&session, &stdin, &waiting, control);
                        continue;
                    }
                    for event in parser.value(&v) {
                        match &event {
                            AgentEvent::ConversationId(_) => {
                                started.store(true, std::sync::atomic::Ordering::Relaxed)
                            }
                            AgentEvent::Done(_) => {
                                let mut list = unanswered.lock().unwrap_or_else(|e| e.into_inner());
                                if !list.is_empty() {
                                    list.remove(0);
                                }
                            }
                            _ => {}
                        }
                        let done = matches!(event, AgentEvent::Done(_));
                        session.apply(event);
                        if done {
                            session.set_status(Status::Idle);
                        }
                    }
                }
            })
        };

        let mut writer = Writer {
            stdin: stdin.clone(),
            unanswered: unanswered.clone(),
            // "Browser tools only" never prompts: no control protocol needed.
            initialized: !self.access.permission_mode.prompts(),
        };
        for text in replay {
            session.set_status(Status::Running);
            writer.message(&session, &text).await;
        }

        // Relay user messages until stopped or the process ends.
        let exit = loop {
            tokio::select! {
                _ = cancel.cancelled() => break None,
                status = child.wait() => break Some(status),
                text = input.recv() => {
                    let Some(text) = text else { break None };
                    session.set_status(Status::Running);
                    writer.message(&session, &text).await;
                }
            }
        };

        match exit {
            None => {
                let was_running = session.status() == Status::Running;
                // Close stdin first (claude exits on EOF), then kill the whole tree: killing
                // only `cmd.exe` would leave node.exe running and holding stdout open.
                stdin.lock().await.take();
                kill_tree(&mut child, &tree).await;
                join_reader(reader).await;
                if was_running {
                    session.apply(AgentEvent::Done("stopped".into()));
                }
                Exit::Stopped
            }
            Some(status) => {
                join_reader(reader).await;
                tree.kill(); // helpers the agent left behind
                let unanswered =
                    std::mem::take(&mut *unanswered.lock().unwrap_or_else(|e| e.into_inner()));
                Exit::Exited {
                    status,
                    stderr: stderr.text(),
                    started: started.load(std::sync::atomic::Ordering::Relaxed),
                    unanswered,
                }
            }
        }
    }
}

/// Answer a control request without blocking the reader: a permission request waits for a
/// person, possibly for minutes.
fn handle_control(
    session: &Arc<Session>,
    stdin: &Stdin,
    waiting: &Arc<Mutex<HashMap<String, String>>>,
    control: Control,
) {
    match control {
        Control::CanUseTool {
            request_id,
            tool_name,
            input,
            permission,
        } => {
            let ask = session.ask_permission(*permission);
            let (stdin, waiting) = (stdin.clone(), waiting.clone());
            tokio::spawn(async move {
                let decision = match ask {
                    Ask::Answered(decision) => decision,
                    Ask::Pending { request, answer } => {
                        lock(&waiting).insert(request_id.clone(), request.id);
                        let decision = answer.await.unwrap_or(Decision::Cancelled);
                        lock(&waiting).remove(&request_id);
                        decision
                    }
                };
                let line = permission_response(&request_id, &tool_name, &input, &decision);
                // Fails only when claude is gone; nothing to answer then.
                let _ = write_line(&stdin, &line).await;
            });
        }
        Control::Cancel(request_id) => {
            if let Some(id) = lock(waiting).get(&request_id).cloned() {
                session.cancel_permission(&id);
            }
        }
        Control::Unsupported {
            request_id,
            subtype,
        } => {
            let stdin = stdin.clone();
            tokio::spawn(async move {
                let line = error_response(
                    &request_id,
                    &format!("{subtype} is not supported by BrowserMCP"),
                );
                let _ = write_line(&stdin, &line).await;
            });
        }
        Control::Response => {}
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `--resume` failed because Claude Code no longer has the conversation ("No conversation
/// found with session ID: …"); other startup failures are reported as errors.
fn lost_conversation(stderr: &str) -> bool {
    stderr
        .to_ascii_lowercase()
        .contains("no conversation found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::access::{CodexApproval, CodexSandbox};

    /// Lines as printed by `claude` 2.1.x with `--output-format stream-json --verbose
    /// --include-partial-messages` (trimmed to the fields we read).
    const SAMPLE: &[&str] = &[
        r#"{"type":"system","subtype":"init","cwd":"C:\\x","session_id":"5f0c","tools":["mcp__browsermcp__system_ping"],"mcp_servers":[{"name":"browsermcp","status":"connected"}],"model":"claude-opus-5-5","permissionMode":"dontAsk"}"#,
        r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1"}},"session_id":"5f0c"}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Pinging"}},"session_id":"5f0c"}"#,
        r#"{"type":"assistant","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"Pinging"},{"type":"tool_use","id":"toolu_1","name":"mcp__browsermcp__system_ping","input":{}}]},"session_id":"5f0c"}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":[{"type":"text","text":"{\"name\":\"BrowserMCP\",\"version\":\"0.1.0\"}"}]}]},"session_id":"5f0c"}"#,
        r#"{"type":"assistant","message":{"id":"msg_2","role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"mcp__browsermcp__tabs_open","input":{"url":"https://a.test/"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_2","type":"tool_result","content":"{\"tab\":4}"}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_9","type":"tool_result","content":"boom","is_error":true}]}}"#,
        r#"{"type":"assistant","message":{"id":"msg_3","role":"assistant","content":[{"type":"text","text":"ok"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":4000,"num_turns":2,"result":"ok","session_id":"5f0c","total_cost_usd":0.01}"#,
    ];

    fn access(mode: PermissionMode) -> Access {
        Access {
            permission_mode: mode,
            cwd: PathBuf::from("C:\\w"),
            add_dirs: vec![PathBuf::from("D:\\docs")],
            sandbox: CodexSandbox::WorkspaceWrite,
            approval_policy: CodexApproval::OnRequest,
        }
    }

    #[test]
    fn parses_recorded_stream_json() {
        let mut parser = Parser::default();
        let events: Vec<AgentEvent> = SAMPLE.iter().flat_map(|l| parser.line(l)).collect();
        assert_eq!(
            events,
            vec![
                AgentEvent::ConversationId("5f0c".into()),
                AgentEvent::TextDelta("Pinging".into()),
                AgentEvent::Text("Pinging".into()),
                AgentEvent::ToolCall {
                    name: "system_ping".into(),
                    args: json!({})
                },
                AgentEvent::ToolResult {
                    name: "system_ping".into(),
                    ok: true,
                    summary: r#"{"name":"BrowserMCP","version":"0.1.0"}"#.into(),
                    output: Some(json!({"name": "BrowserMCP", "version": "0.1.0"})),
                },
                AgentEvent::ToolCall {
                    name: "tabs_open".into(),
                    args: json!({"url": "https://a.test/"})
                },
                AgentEvent::ToolResult {
                    name: "tabs_open".into(),
                    ok: true,
                    summary: r#"{"tab":4}"#.into(),
                    output: Some(json!({"tab": 4})),
                },
                AgentEvent::ToolResult {
                    name: "tool".into(),
                    ok: false,
                    summary: "boom".into(),
                    output: None
                },
                AgentEvent::Text("ok".into()),
                AgentEvent::Done("end_turn".into()),
            ]
        );
    }

    #[test]
    fn init_warns_when_mcp_not_connected_and_errors_end_the_turn() {
        let mut parser = Parser::default();
        let events = parser.line(
            r#"{"type":"system","subtype":"init","mcp_servers":[{"name":"browsermcp","status":"failed"}]}"#,
        );
        assert!(matches!(&events[..], [AgentEvent::Notice(n)] if n.contains("failed")));
        let events =
            parser.line(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#);
        assert_eq!(
            events,
            vec![AgentEvent::Done("error: error_max_turns".into())]
        );
        assert!(parser.line("not json").is_empty());
    }

    #[test]
    fn control_protocol_requests_and_answers() {
        let v = json!({"type": "control_request", "request_id": "r1", "request": {
            "subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"},
            "tool_use_id": "toolu_1", "title": "Claude wants to run ls",
            "decision_reason": {"type": "rule", "reason": "Bash needs approval"}}});
        let Some(Control::CanUseTool {
            request_id,
            tool_name,
            input,
            permission,
        }) = control(&v)
        else {
            panic!("not a permission request: {v}")
        };
        assert_eq!((request_id.as_str(), tool_name.as_str()), ("r1", "Bash"));
        assert_eq!(permission.title.as_deref(), Some("Claude wants to run ls"));
        assert_eq!(permission.reason.as_deref(), Some("Bash needs approval"));
        assert_eq!(permission.kind, PermissionKind::Tool);

        let allow: Value =
            serde_json::from_str(&permission_response("r1", "Bash", &input, &Decision::Allow))
                .unwrap();
        assert_eq!(allow["type"], "control_response");
        assert_eq!(allow["response"]["subtype"], "success");
        assert_eq!(allow["response"]["request_id"], "r1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allow["response"]["response"]["updatedInput"]["command"],
            "ls"
        );

        let always: Value = serde_json::from_str(&permission_response(
            "r1",
            "Bash",
            &input,
            &Decision::AllowAlways,
        ))
        .unwrap();
        let rule = &always["response"]["response"]["updatedPermissions"][0];
        assert_eq!(rule["rules"][0]["toolName"], "Bash");
        assert_eq!(rule["destination"], "session");

        let deny: Value = serde_json::from_str(&permission_response(
            "r1",
            "Bash",
            &input,
            &Decision::Deny {
                reason: Some("use dir".into()),
            },
        ))
        .unwrap();
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(
            deny["response"]["response"]["message"],
            "The user denied this action: use dir"
        );

        assert_eq!(
            control(&json!({"type": "control_cancel_request", "request_id": "r1"})),
            Some(Control::Cancel("r1".into()))
        );
        assert!(matches!(
            control(&json!({"type": "control_request", "request_id": "r2", "request": {"subtype": "hook_callback"}})),
            Some(Control::Unsupported { subtype, .. }) if subtype == "hook_callback"
        ));
        assert_eq!(
            control(&json!({"type": "control_response"})),
            Some(Control::Response)
        );
        assert_eq!(control(&json!({"type": "assistant"})), None);
        let err: Value = serde_json::from_str(&error_response("r2", "nope")).unwrap();
        assert_eq!(err["response"]["subtype"], "error");
    }

    /// Regression: with an npm-style `claude.cmd` shim the child is cmd.exe and the real work
    /// runs in a grandchild that holds stdout open. Stopping must kill the whole tree, so the
    /// session task ends promptly (stdout EOF, not the reader timeout), reaches Stopped and
    /// removes its MCP config.
    #[cfg(windows)]
    #[tokio::test]
    async fn stop_kills_the_agent_behind_a_cmd_shim() {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("claude.cmd");
        std::fs::write(&shim, "@ping -n 600 127.0.0.1\r\n").unwrap();
        let (session, input) = Session::new(
            "cc-test".into(),
            "claude-code".into(),
            None,
            "#888888".into(),
            bmcp_core::EventBus::new(),
            None,
        );
        let session = Arc::new(session);
        let mcp_config = dir.path().join("s.mcp.json");
        let mut access = access(PermissionMode::Ask);
        access.cwd = dir.path().to_path_buf();
        access.add_dirs.clear();
        let agent = ClaudeCode {
            session: session.clone(),
            binary: shim,
            model: None,
            mcp_url: "http://127.0.0.1:9/mcp?agent=cc-test".into(),
            mcp_config: mcp_config.clone(),
            access,
            resume: None,
        };
        let task = tokio::spawn(agent.run(input));
        // Give cmd.exe time to start ping.exe.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(mcp_config.exists());

        let stopped = Instant::now();
        session.cancel();
        tokio::time::timeout(Duration::from_secs(120), task)
            .await
            .expect("the session task must finish after stop")
            .unwrap();
        // Well below the (test) reader grace of 60 s (ping itself would run ~10 min): stdout
        // closed because ping.exe died too, not because the grace ran out. Generous margin
        // for a busy CI/dev machine.
        assert!(
            stopped.elapsed() < Duration::from_secs(40),
            "took {:?}",
            stopped.elapsed()
        );
        assert_eq!(session.status(), Status::Stopped);
        assert!(!mcp_config.exists());
    }

    #[test]
    fn input_line_and_args() {
        let line = user_line("hi \"there\"");
        assert!(line.ends_with('\n'));
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["message"]["content"][0]["text"], "hi \"there\"");

        let ask = access(PermissionMode::Ask);
        let args_for = |access: &Access, resume: Option<&str>| {
            args(&Args {
                mcp_config: "C:\\d\\s.mcp.json",
                model: Some("sonnet"),
                append_prompt: "prompt",
                access,
                resume,
            })
            .join(" ")
        };
        let joined = args_for(&ask, Some("5f0c"));
        for flag in [
            "-p",
            "--input-format stream-json",
            "--output-format stream-json",
            "--verbose",
            "--strict-mcp-config",
            "--mcp-config C:\\d\\s.mcp.json",
            "--allowedTools mcp__browsermcp",
            "--permission-prompt-tool stdio",
            "--add-dir D:\\docs",
            "--model sonnet",
            "--resume=5f0c",
        ] {
            assert!(joined.contains(flag), "missing {flag} in {joined}");
        }
        // "Ask me" is Claude Code's default mode: no --permission-mode at all.
        assert!(!joined.contains("--permission-mode"), "{joined}");

        let browser_only = args_for(&access(PermissionMode::BrowserOnly), None);
        assert!(browser_only.contains("--permission-mode dontAsk"));
        assert!(!browser_only.contains("--permission-prompt-tool"));
        assert!(!browser_only.contains("--resume"));
        let full = args_for(&access(PermissionMode::FullAccess), None);
        assert!(full.contains("--permission-mode bypassPermissions"));
        assert!(full.contains("--allow-dangerously-skip-permissions"));
        assert!(
            args_for(&access(PermissionMode::AcceptEdits), None)
                .contains("--permission-mode acceptEdits")
        );
        assert!(args_for(&access(PermissionMode::Plan), None).contains("--permission-mode plan"));

        assert_eq!(
            mcp_config("http://127.0.0.1:1/mcp?agent=a")["mcpServers"]["browsermcp"]["url"],
            "http://127.0.0.1:1/mcp?agent=a"
        );
        let init: Value = serde_json::from_str(initialize_line().trim()).unwrap();
        assert_eq!(init["request"]["subtype"], "initialize");
    }
}
