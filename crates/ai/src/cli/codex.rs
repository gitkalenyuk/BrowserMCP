//! Codex as an agent.
//!
//! Preferred: `codex app-server` — JSON-RPC over stdio, one JSON object per line, without the
//! `"jsonrpc"` field (protocol from `codex app-server generate-json-schema`, codex-cli 0.166):
//!
//! 1. `initialize {clientInfo}` → result; then the `initialized` notification;
//! 2. `thread/start {cwd, approvalPolicy, sandbox, model?, developerInstructions}` → `{thread: {id}}`,
//!    or `thread/resume {threadId, …same}` to continue a stopped chat (falls back to a new
//!    thread, with a notice, when Codex no longer has it);
//! 3. per user message `turn/start {threadId, input: [{type: "text", text}]}` → `{turn: {id}}`;
//!    notifications `item/agentMessage/delta`, `item/started`, `item/completed`
//!    (`agentMessage`, `mcpToolCall`, `commandExecution`, `fileChange`), `turn/completed {turn: {status}}`;
//! 4. approvals: the server asks `item/commandExecution/requestApproval {command, cwd, reason}`
//!    and `item/fileChange/requestApproval {itemId, reason}` (older: `execCommandApproval`,
//!    `applyPatchApproval`); they become approval cards ([`Session::ask_permission`]) and the
//!    answer is `{decision: accept | acceptForSession | decline | cancel}`;
//! 5. stop: `turn/interrupt {threadId, turnId}`, then kill; ai.session_interrupt sends only
//!    `turn/interrupt` (the thread and process stay);
//! 6. a message sent while a turn runs: `turn/steer {threadId, input, expectedTurnId}` adds it
//!    to that turn; when the server does not have it (or the turn just ended) the message is
//!    held and starts the next turn as soon as this one completes;
//! 7. the model: `turn/start {model}` on every turn (ai.session_set_model takes effect with
//!    the next one); `model/list` lists them (ai.models). A named model sticks on Codex's
//!    side, so back to the default the next turn names the `isDefault` model of `model/list`;
//! 8. editing a message: the next run resumes the thread and sends `thread/rollback
//!    {threadId, numTurns}` (falls back to a new thread, with a notice, when that fails).
//!
//! The methods of 6–8 come from the app-server protocol of newer Codex versions and were not
//! tried against a real Codex (none is installed here); the tests use a fake app-server.
//!
//! The browser's MCP server is passed as a config override:
//! `-c mcp_servers.browsermcp.url="http://127.0.0.1:47800/mcp?agent=<id>"`. Sandbox and
//! approval policy come from the session's [`Access`] (default: workspace-write, on-request).
//!
//! Fallback when the app-server does not start (old Codex): `codex exec --json -` per message
//! (`codex exec resume <thread> --json … -` for the next ones), the prompt written to stdin
//! (`-`), so user text is never parsed as flags. No approvals there (exec cannot ask): only the
//! sandbox applies. That path is untested against a real Codex.
//!
//! Every child runs in a [`ProcessTree`]: stopping kills `codex.cmd`'s grandchildren too.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot};

use super::access::Access;
use super::permission::{Ask, Decision, NewPermission, PermissionKind};
use super::{ProcessTree, StderrTail, command, join_reader, kill_tree, spawn};
use crate::session::{AgentEvent, Input, Session, Status, summarize};

const SERVER: &str = "browsermcp";
/// How long a JSON-RPC request may take (thread/start loads MCP servers).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Codex {
    pub session: Arc<Session>,
    pub binary: PathBuf,
    /// Used when the chat has no model of its own (the provider's default).
    pub model: Option<String>,
    pub mcp_url: String,
    /// Sandbox, approval policy and working folder (`access.cwd`).
    pub access: Access,
    /// Codex thread id to continue.
    pub resume: Option<String>,
    /// After an edit: turns to drop from the end of the resumed thread.
    pub rollback: Option<u32>,
}

/// `-c` override that adds the browser's MCP server.
pub fn mcp_override(url: &str) -> String {
    format!("mcp_servers.{SERVER}.url=\"{url}\"")
}

/// `thread/start` parameters.
pub fn thread_start_params(model: Option<&str>, instructions: &str, access: &Access) -> Value {
    let mut params = json!({
        "cwd": access.cwd.to_string_lossy(),
        "approvalPolicy": access.approval_policy.wire(),
        "sandbox": access.sandbox.wire(),
        "developerInstructions": instructions,
    });
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        params["model"] = json!(model);
    }
    params
}

/// `thread/resume` parameters: the thread, with this session's settings again.
pub fn thread_resume_params(
    thread: &str,
    model: Option<&str>,
    instructions: &str,
    access: &Access,
) -> Value {
    let mut params = thread_start_params(model, instructions, access);
    params["threadId"] = json!(thread);
    // Only the thread's state is needed, not its whole history.
    params["excludeTurns"] = json!(true);
    params
}

/// Which answer format a server request expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalFormat {
    /// v2 `item/*/requestApproval`: `accept | acceptForSession | decline | cancel`.
    V2,
    /// `execCommandApproval` / `applyPatchApproval`: `approved | approved_for_session | denied | abort`.
    Legacy,
}

/// A server request that asks for approval, as a permission request. `file_changes`: the
/// files of `fileChange` items seen so far (the approval request itself names only the item).
pub fn approval_request(
    method: &str,
    params: &Value,
    file_changes: &HashMap<String, Value>,
) -> Option<(NewPermission, ApprovalFormat)> {
    let reason = params["reason"]
        .as_str()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    let command = |format| {
        let command = match &params["command"] {
            Value::Array(parts) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            other => other.as_str().unwrap_or_default().to_string(),
        };
        Some((
            NewPermission {
                tool: "shell".into(),
                kind: PermissionKind::Command,
                title: None,
                reason: reason.clone(),
                input: json!({ "command": command, "cwd": params["cwd"] }),
            },
            format,
        ))
    };
    let files = |format| {
        let item = params["itemId"].as_str().or(params["callId"].as_str());
        let mut input = json!({});
        if let Some(files) = item.and_then(|id| file_changes.get(id)) {
            input["files"] = files.clone();
        } else if let Some(changes) = params["fileChanges"].as_object() {
            input["files"] = json!(changes.keys().collect::<Vec<_>>());
        }
        if let Some(root) = params["grantRoot"].as_str() {
            input["grant_root"] = json!(root);
        }
        Some((
            NewPermission {
                tool: "file_change".into(),
                kind: PermissionKind::FileChange,
                title: None,
                reason: reason.clone(),
                input,
            },
            format,
        ))
    };
    match method {
        "item/commandExecution/requestApproval" => command(ApprovalFormat::V2),
        "execCommandApproval" => command(ApprovalFormat::Legacy),
        "item/fileChange/requestApproval" => files(ApprovalFormat::V2),
        "applyPatchApproval" => files(ApprovalFormat::Legacy),
        _ => None,
    }
}

/// The answer to an approval request.
pub fn approval_answer(format: ApprovalFormat, decision: &Decision) -> Value {
    let decision = match (format, decision) {
        (ApprovalFormat::V2, Decision::Allow) => "accept",
        (ApprovalFormat::V2, Decision::AllowAlways) => "acceptForSession",
        (ApprovalFormat::V2, Decision::Deny { .. }) => "decline",
        (ApprovalFormat::V2, Decision::Cancelled) => "cancel",
        (ApprovalFormat::Legacy, Decision::Allow) => "approved",
        (ApprovalFormat::Legacy, Decision::AllowAlways) => "approved_for_session",
        (ApprovalFormat::Legacy, Decision::Deny { .. }) => "denied",
        (ApprovalFormat::Legacy, Decision::Cancelled) => "abort",
    };
    json!({ "decision": decision })
}

/// The files of a `fileChange` item (`item/started`), for its approval card.
fn file_change_paths(item: &Value) -> Option<(String, Value)> {
    if item["type"] != "fileChange" {
        return None;
    }
    let paths: Vec<&str> = item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["path"].as_str())
        .collect();
    Some((item["id"].as_str()?.to_string(), json!(paths)))
}

/// `turn/start` parameters; `model` applies to this turn and the next ones.
pub fn turn_start_params(thread: &str, text: &str, model: Option<&str>) -> Value {
    let mut params = json!({ "threadId": thread, "input": text_input(text) });
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        params["model"] = json!(model);
    }
    params
}

fn text_input(text: &str) -> Value {
    json!([{ "type": "text", "text": text, "text_elements": [] }])
}

/// `turn/steer` parameters: add `text` to the running turn `turn`.
pub fn turn_steer_params(thread: &str, turn: &str, text: &str) -> Value {
    json!({ "threadId": thread, "input": text_input(text), "expectedTurnId": turn })
}

/// `thread/rollback` parameters: drop the last `turns` turns of the thread.
pub fn rollback_params(thread: &str, turns: u32) -> Value {
    json!({ "threadId": thread, "numTurns": turns })
}

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = -32601;

/// One model from `model/list`: `(id, name, description, is_default)`. Hidden ones are
/// left out.
pub fn parse_models(result: &Value) -> Vec<(String, String, Option<String>, bool)> {
    let list = result["data"]
        .as_array()
        .or(result["models"].as_array())
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter(|m| m["hidden"] != true)
        .filter_map(|m| {
            let id = m["model"].as_str().or(m["id"].as_str())?.to_string();
            let name = m["displayName"]
                .as_str()
                .or(m["display_name"].as_str())
                .unwrap_or(&id)
                .to_string();
            let hint = m["description"]
                .as_str()
                .filter(|d| !d.is_empty())
                .map(str::to_string);
            Some((id, name, hint, m["isDefault"] == true))
        })
        .collect()
}

/// Ask a `codex app-server` for its models (`initialize`, then `model/list`); no thread is
/// started, nothing is sent to a model.
pub async fn list_models(binary: &std::path::Path, cwd: &std::path::Path) -> Result<Value, String> {
    let mut cmd = command(binary, cwd);
    cmd.arg("app-server");
    let (mut child, tree) = spawn(&mut cmd).map_err(|e| format!("cannot start codex: {e}"))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("codex has no stdio".into());
    };
    if let Some(stderr) = child.stderr.take() {
        StderrTail::collect(stderr, "codex model/list");
    }
    let talk = async {
        let mut lines = BufReader::new(stdout).lines();
        let mut send = async |v: Value| {
            stdin
                .write_all(format!("{v}\n").as_bytes())
                .await
                .map_err(|e| e.to_string())
        };
        send(json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "browsermcp", "title": "BrowserMCP", "version": env!("CARGO_PKG_VERSION") } } })).await?;
        send(json!({ "method": "initialized" })).await?;
        send(json!({ "id": 2, "method": "model/list", "params": {} })).await?;
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["id"] == 2 && v["method"].is_null() {
                return match v.get("error") {
                    Some(err) => Err(err["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(v["result"].clone()),
                };
            }
        }
        Err("codex exited".to_string())
    };
    let result = tokio::time::timeout(Duration::from_secs(20), talk)
        .await
        .unwrap_or_else(|_| Err("model/list: no answer from codex".into()));
    kill_tree(&mut child, &tree).await;
    result
}

/// app-server notification → events.
pub fn notification(method: &str, params: &Value) -> Vec<AgentEvent> {
    let item = &params["item"];
    match method {
        "item/agentMessage/delta" => params["delta"]
            .as_str()
            .map(|d| vec![AgentEvent::TextDelta(d.to_string())])
            .unwrap_or_default(),
        "item/started" => match item["type"].as_str().unwrap_or_default() {
            "mcpToolCall" => vec![AgentEvent::ToolCall {
                name: item["tool"].as_str().unwrap_or("tool").to_string(),
                args: item["arguments"].clone(),
            }],
            "commandExecution" => vec![AgentEvent::ToolCall {
                name: "shell".into(),
                args: json!({ "command": item["command"] }),
            }],
            "fileChange" => vec![AgentEvent::ToolCall {
                name: "file_change".into(),
                args: json!({ "files": file_change_paths(item).map(|(_, p)| p) }),
            }],
            _ => Vec::new(),
        },
        "item/completed" => match item["type"].as_str().unwrap_or_default() {
            "agentMessage" => item["text"]
                .as_str()
                .filter(|t| !t.is_empty())
                .map(|t| vec![AgentEvent::Text(t.to_string())])
                .unwrap_or_default(),
            "mcpToolCall" => {
                let ok = item["status"] == "completed" && item["error"].is_null();
                let (summary, output) = if ok {
                    mcp_result(&item["result"])
                } else {
                    (summarize(&item["error"]["message"]), None)
                };
                vec![AgentEvent::ToolResult {
                    name: item["tool"].as_str().unwrap_or("tool").to_string(),
                    ok,
                    summary,
                    output,
                }]
            }
            "commandExecution" => {
                let ok = item["exitCode"] == 0;
                // A refused command has no output: say why it did not run.
                let summary = match (item["status"].as_str(), &item["aggregatedOutput"]) {
                    (Some("declined"), _) => "Declined: the command was not approved".to_string(),
                    (_, Value::Null) if !ok => match item["exitCode"].as_i64() {
                        Some(code) => format!("Exited with code {code}"),
                        None => "The command failed".to_string(),
                    },
                    (_, output) => summarize(output),
                };
                vec![AgentEvent::ToolResult {
                    name: "shell".into(),
                    ok,
                    summary,
                    output: None,
                }]
            }
            "fileChange" => {
                let status = item["status"].as_str().unwrap_or_default();
                let summary = match status {
                    "completed" => "Files changed".to_string(),
                    "declined" => "Declined: the change was not approved".to_string(),
                    other => format!("The change was not applied ({other})"),
                };
                vec![AgentEvent::ToolResult {
                    name: "file_change".into(),
                    ok: status == "completed",
                    summary,
                    output: None,
                }]
            }
            _ => Vec::new(),
        },
        "turn/completed" => {
            let turn = &params["turn"];
            let status = turn["status"].as_str().unwrap_or("completed");
            let reason = match status {
                "completed" => "end_turn".to_string(),
                "interrupted" => "stopped".to_string(),
                other => match turn["error"]["message"].as_str() {
                    Some(message) => format!("error: {message}"),
                    None => format!("error: {other}"),
                },
            };
            // A turn of the thread (what thread/rollback counts), whatever its status.
            let id = turn["id"].as_str().unwrap_or("turn").to_string();
            vec![AgentEvent::Turn(id), AgentEvent::Done(reason)]
        }
        "error" => {
            let message = params["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            vec![AgentEvent::Notice(format!("Codex: {message}"))]
        }
        _ => Vec::new(),
    }
}

/// An MCP tool result: summary text, and the JSON output when there is one.
fn mcp_result(result: &Value) -> (String, Option<Value>) {
    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let output = match &result["structuredContent"] {
        Value::Null => serde_json::from_str(&text).ok(),
        structured => Some(structured.clone()),
    };
    (summarize(&Value::String(text)), output)
}

/// Answer for a server request that is not an approval (those go to the user, see
/// [`approval_request`]). MCP elicitations (tool-call confirmations) for our server are
/// accepted: the browser's tools are always allowed.
pub fn server_request_answer(method: &str) -> Result<Value, Value> {
    match method {
        "mcpServer/elicitation/request" => Ok(json!({ "action": "accept", "content": {} })),
        _ => Err(
            json!({ "code": -32601, "message": format!("{method} is not supported by BrowserMCP") }),
        ),
    }
}

/// The [`AgentEvent::Turn`] of a `codex exec` turn.
const EXEC_TURN: &str = "exec";

/// `codex exec --json` event line → events (fallback mode).
pub fn exec_event(v: &Value, thread: &mut Option<String>) -> Vec<AgentEvent> {
    let item = &v["item"];
    match v["type"].as_str().unwrap_or_default() {
        "thread.started" => {
            *thread = v["thread_id"].as_str().map(str::to_string);
            Vec::new()
        }
        "item.started" if item["type"] == "mcp_tool_call" => vec![AgentEvent::ToolCall {
            name: item["tool"].as_str().unwrap_or("tool").to_string(),
            args: item["arguments"].clone(),
        }],
        "item.completed" => match item["type"].as_str().unwrap_or_default() {
            "agent_message" => item["text"]
                .as_str()
                .map(|t| vec![AgentEvent::Text(t.to_string())])
                .unwrap_or_default(),
            "mcp_tool_call" => {
                let ok = item["status"] == "completed" && item["error"].is_null();
                let (summary, output) = if ok {
                    mcp_result(&item["result"])
                } else {
                    (summarize(&item["error"]), None)
                };
                vec![AgentEvent::ToolResult {
                    name: item["tool"].as_str().unwrap_or("tool").to_string(),
                    ok,
                    summary,
                    output,
                }]
            }
            _ => Vec::new(),
        },
        // A turn of the thread (`codex exec` names no turn ids).
        "turn.completed" => vec![
            AgentEvent::Turn(EXEC_TURN.into()),
            AgentEvent::Done("end_turn".into()),
        ],
        "turn.failed" => vec![
            AgentEvent::Turn(EXEC_TURN.into()),
            AgentEvent::Done(format!(
                "error: {}",
                v["error"]["message"].as_str().unwrap_or("turn failed")
            )),
        ],
        "error" => vec![AgentEvent::Notice(format!(
            "Codex: {}",
            v["message"].as_str().unwrap_or("error")
        ))],
        _ => Vec::new(),
    }
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>>;

/// The client side of the app-server connection.
struct Rpc {
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicI64,
}

impl Rpc {
    async fn write(&self, message: &Value) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(format!("{message}\n").as_bytes()).await?;
        stdin.flush().await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_value(method, params)
            .await
            .map_err(|error| format!("{method}: {}", error["message"].as_str().unwrap_or("error")))
    }

    /// Like [`Rpc::request`], with the JSON-RPC error object (`{code, message}`) on failure.
    async fn request_value(&self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if let Err(e) = self
            .write(&json!({ "id": id, "method": method, "params": params }))
            .await
        {
            return Err(json!({ "message": format!("cannot write to codex: {e}") }));
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Err(_) => Err(json!({ "message": "no answer from codex" })),
            Ok(Err(_)) => Err(json!({ "message": "codex exited" })),
            Ok(Ok(result)) => result,
        }
    }

    async fn notify(&self, method: &str) -> std::io::Result<()> {
        self.write(&json!({ "method": method })).await
    }
}

/// The model Codex uses by default (`isDefault` in model/list); `None` when it does not say.
async fn codex_default_model(rpc: &Rpc) -> Option<String> {
    match rpc.request("model/list", json!({})).await {
        Ok(result) => {
            let default = parse_models(&result)
                .into_iter()
                .find(|(_, _, _, is_default)| *is_default)
                .map(|(id, ..)| id);
            if default.is_none() {
                tracing::warn!("codex model/list names no default model");
            }
            default
        }
        Err(err) => {
            tracing::warn!(%err, "codex model/list failed: the chat keeps its last model");
            None
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `codex exec` arguments for one message. The prompt itself is read from stdin (`-`).
pub fn exec_args(
    thread: Option<&str>,
    mcp_url: &str,
    model: Option<&str>,
    access: &Access,
) -> Vec<String> {
    let mut args = vec!["exec".to_string()];
    if let Some(thread) = thread {
        args.extend(["resume".into(), thread.into()]);
    }
    args.extend([
        "--json".into(),
        "--skip-git-repo-check".into(),
        "-c".into(),
        mcp_override(mcp_url),
        // `codex exec` cannot ask: only the sandbox applies.
        "-c".into(),
        format!("sandbox_mode=\"{}\"", access.sandbox.wire()),
    ]);
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        args.extend(["-m".into(), model.into()]);
    }
    // `-`: "read the prompt from stdin" (`codex exec --help`), for `exec` and `exec resume`.
    args.push("-".into());
    args
}

impl Codex {
    fn spawn_app_server(&self) -> std::io::Result<(Child, ProcessTree)> {
        let mut cmd = command(&self.binary, &self.access.cwd);
        cmd.arg("app-server")
            .args(["-c", &mcp_override(&self.mcp_url)]);
        spawn(&mut cmd)
    }

    /// The session task.
    pub async fn run(self, input: mpsc::UnboundedReceiver<Input>) {
        let session = self.session.clone();
        self.run_inner(input).await;
        // Codex is gone: nobody waits for answers to its approval requests any more.
        session.cancel_permissions();
    }

    async fn run_inner(&self, input: mpsc::UnboundedReceiver<Input>) {
        let session = self.session.clone();
        match self.spawn_app_server() {
            Ok((child, tree)) => match self.app_server(child, tree, input).await {
                Ok(()) => {}
                Err((message, Some(input))) => {
                    // Could not even start a thread: try the older `codex exec` mode.
                    tracing::warn!(%message, "codex app-server failed; falling back to codex exec");
                    session.apply(AgentEvent::Notice(format!(
                        "codex app-server failed ({message}); using `codex exec --json`"
                    )));
                    self.exec_mode(input).await;
                }
                Err((message, None)) => {
                    session.apply(AgentEvent::Notice(message.clone()));
                    session.apply(AgentEvent::Done(format!("error: {message}")));
                    session.set_status(Status::Failed);
                }
            },
            Err(err) => {
                let message = format!("cannot start {}: {err}", self.binary.display());
                session.apply(AgentEvent::Notice(message.clone()));
                session.apply(AgentEvent::Done(format!("error: {message}")));
                session.set_status(Status::Failed);
            }
        }
    }

    /// Drive `codex app-server`. On a startup failure the input queue is handed back so the
    /// caller can fall back to exec mode.
    async fn app_server(
        &self,
        mut child: Child,
        tree: ProcessTree,
        mut input: mpsc::UnboundedReceiver<Input>,
    ) -> Result<(), (String, Option<mpsc::UnboundedReceiver<Input>>)> {
        let session = self.session.clone();
        let cancel = session.cancel_token();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(("codex has no stdio".into(), Some(input)));
        };
        let stderr = StderrTail::collect(stderr, "codex");
        let rpc = Arc::new(Rpc {
            stdin: tokio::sync::Mutex::new(stdin),
            pending: Pending::default(),
            next_id: AtomicI64::new(0),
        });
        let turn: Arc<Mutex<Option<String>>> = Arc::default();
        // The reader tells the turn loop when a turn ended (held messages start the next).
        let (turn_end_tx, mut turn_end) = mpsc::unbounded_channel::<()>();
        // Turns ended so far (tells a late `turn/start` answer from a running turn).
        let ended = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // stdout: responses → pending requests; notifications → events; server requests → answers.
        let reader = {
            let (rpc, session, turn, ended) =
                (rpc.clone(), session.clone(), turn.clone(), ended.clone());
            let stdout = BufReader::new(stdout);
            tokio::spawn(async move {
                // fileChange item id → its files, for the approval card.
                let mut file_changes: HashMap<String, Value> = HashMap::new();
                let mut lines = stdout.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let method = msg["method"].as_str();
                    match (method, msg.get("id")) {
                        (None, Some(id)) => {
                            if let Some(tx) =
                                id.as_i64().and_then(|id| lock(&rpc.pending).remove(&id))
                            {
                                let result = match msg.get("error") {
                                    Some(err) => Err(err.clone()),
                                    None => Ok(msg["result"].clone()),
                                };
                                let _ = tx.send(result);
                            }
                        }
                        (Some(method), Some(id)) => {
                            if let Some((permission, format)) =
                                approval_request(method, &msg["params"], &file_changes)
                            {
                                // Waits for a person: never block the reader on it.
                                let ask = session.ask_permission(permission);
                                let (rpc, id) = (rpc.clone(), id.clone());
                                tokio::spawn(async move {
                                    let decision = match ask {
                                        Ask::Answered(decision) => decision,
                                        Ask::Pending { answer, .. } => {
                                            answer.await.unwrap_or(Decision::Cancelled)
                                        }
                                    };
                                    let result = approval_answer(format, &decision);
                                    let _ = rpc.write(&json!({ "id": id, "result": result })).await;
                                });
                                continue;
                            }
                            let answer = match server_request_answer(method) {
                                Ok(result) => json!({ "id": id, "result": result }),
                                Err(error) => json!({ "id": id, "error": error }),
                            };
                            let _ = rpc.write(&answer).await;
                        }
                        (Some(method), None) => {
                            if method == "turn/started" {
                                *lock(&turn) =
                                    msg["params"]["turn"]["id"].as_str().map(str::to_string);
                            }
                            if method == "item/started"
                                && let Some((item, files)) =
                                    file_change_paths(&msg["params"]["item"])
                            {
                                file_changes.insert(item, files);
                            }
                            for event in notification(method, &msg["params"]) {
                                let done = matches!(event, AgentEvent::Done(_));
                                if done {
                                    let mut current = lock(&turn);
                                    *current = None;
                                    ended.fetch_add(1, Ordering::Relaxed);
                                }
                                // Done: idle, or running when held messages wait.
                                session.apply(event);
                                if done {
                                    let _ = turn_end_tx.send(());
                                }
                            }
                        }
                        (None, None) => {}
                    }
                }
                // Codex exited: fail whatever is still waiting.
                lock(&rpc.pending).clear();
            })
        };

        // The model named on thread/start or turn/start sticks for the following turns (a
        // resumed thread may still have the one an earlier run named). Once one may be in
        // effect, going back to the default has to name the default model explicitly: the
        // `isDefault` entry of model/list, asked once per run when needed.
        let first_model = session.model().or_else(|| self.model.clone());
        let named =
            std::sync::atomic::AtomicBool::new(first_model.is_some() || self.resume.is_some());
        let default_model: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::new();

        // Handshake.
        let started = async {
            rpc.request(
                "initialize",
                json!({ "clientInfo": { "name": "browsermcp", "title": "BrowserMCP", "version": env!("CARGO_PKG_VERSION") } }),
            )
            .await?;
            rpc.notify("initialized").await.map_err(|e| e.to_string())?;
            let instructions = crate::agent::system_prompt(&session.id);
            let model = first_model.as_deref();
            if let Some(thread) = &self.resume {
                let params = thread_resume_params(thread, model, &instructions, &self.access);
                match rpc.request("thread/resume", params).await {
                    Ok(result) if result["thread"]["id"].is_string() => {
                        let id = result["thread"]["id"]
                            .as_str()
                            .unwrap_or(thread)
                            .to_string();
                        // After an edit: drop the turns from the edited message on.
                        match self.rollback.filter(|n| *n > 0) {
                            None => return Ok(id),
                            Some(n) => match rpc
                                .request("thread/rollback", rollback_params(&id, n))
                                .await
                            {
                                Ok(_) => return Ok(id),
                                Err(err) => tracing::warn!(%err, "codex thread/rollback failed"),
                            },
                        }
                    }
                    Ok(result) => tracing::warn!(%result, "thread/resume returned no thread"),
                    Err(err) => tracing::warn!(%err, "codex thread/resume failed"),
                }
                // Same chat, new Codex thread: say so, the agent lost its context.
                session.apply(AgentEvent::resume_lost("Codex"));
            }
            let params = thread_start_params(model, &instructions, &self.access);
            let result = rpc.request("thread/start", params).await?;
            result["thread"]["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("thread/start returned no thread id: {result}"))
        };
        let thread = tokio::select! {
            _ = cancel.cancelled() => {
                kill_tree(&mut child, &tree).await;
                reader.abort();
                session.apply(AgentEvent::Done("stopped".into()));
                session.set_status(Status::Stopped);
                return Ok(());
            }
            thread = started => match thread {
                Ok(thread) => thread,
                Err(err) => {
                    kill_tree(&mut child, &tree).await;
                    reader.abort();
                    let tail = stderr.text();
                    return Err((format!("{err} {tail}").trim().to_string(), Some(input)));
                }
            },
        };
        session.apply(AgentEvent::ConversationId(thread.clone()));

        // Turns. Messages sent while a turn runs are steered into it, or held for the next.
        let mut held: Vec<(String, Option<usize>)> = Vec::new();
        let mut can_steer = true;
        let start_turn = async |texts: Vec<(String, Option<usize>)>| {
            let indexes: Vec<usize> = texts.iter().filter_map(|(_, i)| *i).collect();
            let text = texts
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>()
                .join("\n\n");
            session.take(&indexes);
            session.set_status(Status::Running);
            // ai.session_set_model: the chat's model, from this turn on.
            let model = match session.model().or_else(|| self.model.clone()) {
                Some(model) => {
                    named.store(true, Ordering::Relaxed);
                    Some(model)
                }
                None if named.load(Ordering::Relaxed) => default_model
                    .get_or_init(|| codex_default_model(&rpc))
                    .await
                    .clone(),
                None => None,
            };
            let params = turn_start_params(&thread, &text, model.as_deref());
            let ended_before = ended.load(Ordering::Relaxed);
            match rpc.request("turn/start", params).await {
                Ok(result) => {
                    // Running from now on, unless `turn/started` came first or the turn is
                    // already over.
                    let mut current = lock(&turn);
                    if current.is_none()
                        && ended.load(Ordering::Relaxed) == ended_before
                        && let Some(id) = result["turn"]["id"].as_str()
                    {
                        *current = Some(id.to_string());
                    }
                }
                Err(err) => {
                    session.apply(AgentEvent::Notice(err.clone()));
                    session.apply(AgentEvent::Done(format!("error: {err}")));
                }
            }
        };
        let exit = loop {
            tokio::select! {
                _ = cancel.cancelled() => break None,
                status = child.wait() => break Some(status),
                Some(()) = turn_end.recv() => {
                    if !held.is_empty() && lock(&turn).is_none() {
                        start_turn(std::mem::take(&mut held)).await;
                    }
                }
                next = input.recv() => {
                    let Some(next) = next else { break None };
                    match next {
                        Input::Message { text, index, .. } => {
                            let running = lock(&turn).clone();
                            let Some(turn_id) = running else {
                                start_turn(vec![(text, index)]).await;
                                continue;
                            };
                            if can_steer {
                                let params = turn_steer_params(&thread, &turn_id, &text);
                                match rpc.request_value("turn/steer", params).await {
                                    Ok(_) => {
                                        session.take(&index.into_iter().collect::<Vec<_>>());
                                        continue;
                                    }
                                    Err(err) => {
                                        if err["code"] == METHOD_NOT_FOUND {
                                            can_steer = false;
                                        }
                                        tracing::debug!(%err, "codex turn/steer refused; holding the message");
                                    }
                                }
                            }
                            held.push((text, index));
                            // The turn may have ended while we asked.
                            if lock(&turn).is_none() {
                                start_turn(std::mem::take(&mut held)).await;
                            }
                        }
                        Input::SetModel(_) => {} // read at the next turn/start
                        Input::Interrupt => {
                            let running = lock(&turn).clone();
                            if let Some(turn_id) = running {
                                let interrupt = rpc.request(
                                    "turn/interrupt",
                                    json!({ "threadId": thread, "turnId": turn_id }),
                                );
                                if let Ok(Err(err)) = tokio::time::timeout(Duration::from_secs(5), interrupt).await {
                                    session.apply(AgentEvent::Notice(err));
                                }
                            }
                        }
                    }
                }
            }
        };

        match exit {
            None => {
                let running = lock(&turn).clone();
                if let Some(turn_id) = &running {
                    let interrupt = rpc.request(
                        "turn/interrupt",
                        json!({ "threadId": thread, "turnId": turn_id }),
                    );
                    let _ = tokio::time::timeout(Duration::from_secs(3), interrupt).await;
                }
                if session.status() == Status::Running {
                    // A turn Codex had started ends here (its turn/completed did not come).
                    if let Some(turn_id) = running {
                        session.apply(AgentEvent::Turn(turn_id));
                    }
                    session.apply(AgentEvent::Done("stopped".into()));
                }
                kill_tree(&mut child, &tree).await;
                reader.abort();
                session.set_status(Status::Stopped);
                Ok(())
            }
            Some(status) => {
                join_reader(reader).await;
                tree.kill(); // helpers codex left behind
                let message = format!(
                    "codex exited ({}): {}",
                    status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|e| e.to_string()),
                    stderr.text()
                );
                Err((message, None))
            }
        }
    }

    /// Fallback: one `codex exec --json` process per user message (untested against real Codex).
    async fn exec_mode(&self, mut input: mpsc::UnboundedReceiver<Input>) {
        let session = self.session.clone();
        let cancel = session.cancel_token();
        let mut thread: Option<String> = self.resume.clone();
        loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => break,
                next = input.recv() => match next { Some(next) => next, None => break },
            };
            // Model changes are read per message; exec mode cannot interrupt a turn.
            let Input::Message { text, index, .. } = next else {
                continue;
            };
            session.take(&index.into_iter().collect::<Vec<_>>());
            session.set_status(Status::Running);
            let model = session.model().or_else(|| self.model.clone());
            let mut cmd = command(&self.binary, &self.access.cwd);
            cmd.args(exec_args(
                thread.as_deref(),
                &self.mcp_url,
                model.as_deref(),
                &self.access,
            ));
            let (mut child, tree) = match spawn(&mut cmd) {
                Ok(spawned) => spawned,
                Err(err) => {
                    session.apply(AgentEvent::Done(format!(
                        "error: cannot start codex exec: {err}"
                    )));
                    session.set_status(Status::Failed);
                    return;
                }
            };
            let (Some(mut stdin), Some(stdout), Some(stderr)) =
                (child.stdin.take(), child.stdout.take(), child.stderr.take())
            else {
                break;
            };
            // Drain stderr: an unread pipe fills up (a few KB) and then blocks codex.
            let stderr = StderrTail::collect(stderr, "codex exec");
            // The prompt goes to stdin (`-` argument), then EOF. From a task, so a long prompt
            // cannot block us while codex is busy writing stdout.
            let writer = tokio::spawn(async move {
                let _ = stdin.write_all(text.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
            let mut lines = BufReader::new(stdout).lines();
            let mut done = false;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        writer.abort();
                        kill_tree(&mut child, &tree).await;
                        session.apply(AgentEvent::Done("stopped".into()));
                        session.set_status(Status::Stopped);
                        return;
                    }
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                        let before = thread.clone();
                        for event in exec_event(&v, &mut thread) {
                            done |= matches!(event, AgentEvent::Done(_));
                            session.apply(event);
                        }
                        if thread != before && let Some(id) = &thread {
                            session.apply(AgentEvent::ConversationId(id.clone()));
                        }
                    }
                }
            }
            writer.abort();
            let status = child.wait().await;
            tree.kill();
            if !done {
                match status {
                    Ok(s) if s.success() => {
                        session.apply(AgentEvent::Turn(EXEC_TURN.into()));
                        session.apply(AgentEvent::Done("end_turn".into()));
                    }
                    status => {
                        let code = status
                            .map(|s| s.to_string())
                            .unwrap_or_else(|e| e.to_string());
                        let message = format!("codex exec exited ({code}): {}", stderr.text());
                        session.apply(AgentEvent::Notice(message.trim().to_string()));
                        session.apply(AgentEvent::Done(format!(
                            "error: codex exec exited ({code})"
                        )));
                    }
                }
            }
            session.settle();
        }
        if session.status() == Status::Running {
            session.apply(AgentEvent::Done("stopped".into()));
        }
        session.set_status(Status::Stopped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_server_notifications() {
        let events: Vec<AgentEvent> = [
            ("turn/started", json!({"threadId": "t", "turn": {"id": "u1", "status": "inProgress", "items": []}})),
            ("item/agentMessage/delta", json!({"threadId": "t", "turnId": "u1", "itemId": "i1", "delta": "Hel"})),
            ("item/started", json!({"threadId": "t", "turnId": "u1", "startedAtMs": 1, "item": {
                "type": "mcpToolCall", "id": "i2", "server": "browsermcp", "tool": "tabs_open",
                "status": "inProgress", "arguments": {"url": "https://a.test/"}}})),
            ("item/completed", json!({"threadId": "t", "turnId": "u1", "completedAtMs": 2, "item": {
                "type": "mcpToolCall", "id": "i2", "server": "browsermcp", "tool": "tabs_open",
                "status": "completed", "arguments": {}, "error": null,
                "result": {"content": [{"type": "text", "text": "{\"tab\":5}"}], "structuredContent": {"tab": 5}}}})),
            ("item/completed", json!({"threadId": "t", "turnId": "u1", "completedAtMs": 3, "item": {
                "type": "agentMessage", "id": "i3", "text": "Hello"}})),
            ("turn/completed", json!({"threadId": "t", "turn": {"id": "u1", "status": "completed", "items": []}})),
            ("turn/completed", json!({"threadId": "t", "turn": {"id": "u2", "status": "failed", "error": {"message": "quota"}}})),
        ]
        .iter()
        .flat_map(|(m, p)| notification(m, p))
        .collect();
        assert_eq!(
            events,
            vec![
                AgentEvent::TextDelta("Hel".into()),
                AgentEvent::ToolCall {
                    name: "tabs_open".into(),
                    args: json!({"url": "https://a.test/"})
                },
                AgentEvent::ToolResult {
                    name: "tabs_open".into(),
                    ok: true,
                    summary: r#"{"tab":5}"#.into(),
                    output: Some(json!({"tab": 5})),
                },
                AgentEvent::Text("Hello".into()),
                AgentEvent::Turn("u1".into()),
                AgentEvent::Done("end_turn".into()),
                AgentEvent::Turn("u2".into()),
                AgentEvent::Done("error: quota".into()),
            ]
        );
    }

    fn access(cwd: &std::path::Path) -> Access {
        crate::cli::access::AccessSettings::default()
            .resolve(cwd)
            .unwrap()
    }

    #[test]
    fn requests_and_params() {
        assert_eq!(
            server_request_answer("mcpServer/elicitation/request").unwrap()["action"],
            "accept"
        );
        assert!(server_request_answer("item/tool/requestUserInput").is_err());
        assert_eq!(
            mcp_override("http://127.0.0.1:1/mcp?agent=codex-1"),
            "mcp_servers.browsermcp.url=\"http://127.0.0.1:1/mcp?agent=codex-1\""
        );
        let dir = tempfile::tempdir().unwrap();
        let mut access = access(dir.path());
        let p = thread_start_params(Some("gpt-x"), "hi", &access);
        assert_eq!(p["approvalPolicy"], "on-request");
        assert_eq!(p["sandbox"], "workspace-write");
        assert_eq!(p["model"], "gpt-x");
        assert_eq!(p["cwd"], dir.path().to_string_lossy().as_ref());
        access.sandbox = crate::cli::access::CodexSandbox::ReadOnly;
        access.approval_policy = crate::cli::access::CodexApproval::Never;
        let p = thread_resume_params("th-1", None, "hi", &access);
        assert_eq!(p["threadId"], "th-1");
        assert_eq!(p["sandbox"], "read-only");
        assert_eq!(p["approvalPolicy"], "never");
        assert!(p.get("model").is_none());
        assert_eq!(
            turn_start_params("t1", "go", None)["input"][0]["text"],
            "go"
        );
        assert!(turn_start_params("t1", "go", None).get("model").is_none());
        assert_eq!(
            turn_start_params("t1", "go", Some("gpt-y"))["model"],
            "gpt-y"
        );
        let steer = turn_steer_params("t1", "u3", "also this");
        assert_eq!(
            (
                steer["expectedTurnId"].as_str(),
                steer["input"][0]["text"].as_str()
            ),
            (Some("u3"), Some("also this"))
        );
        assert_eq!(
            rollback_params("t1", 2),
            json!({"threadId": "t1", "numTurns": 2})
        );
    }

    #[test]
    fn approvals_become_permission_requests() {
        let mut files = HashMap::new();
        let started = json!({"type": "fileChange", "id": "i7", "status": "inProgress",
            "changes": [{"path": "C:\\w\\a.txt", "kind": {"type": "add"}, "diff": "+x"}]});
        let (item, paths) = file_change_paths(&started).unwrap();
        files.insert(item, paths);

        let (cmd, format) = approval_request(
            "item/commandExecution/requestApproval",
            &json!({"threadId": "t", "turnId": "u", "itemId": "i6", "startedAtMs": 1,
                "command": "rm -rf build", "cwd": "C:\\w", "reason": "needs to delete files"}),
            &files,
        )
        .unwrap();
        assert_eq!(format, ApprovalFormat::V2);
        assert_eq!(
            (cmd.tool.as_str(), cmd.kind),
            ("shell", PermissionKind::Command)
        );
        assert_eq!(cmd.input["command"], "rm -rf build");
        assert_eq!(cmd.reason.as_deref(), Some("needs to delete files"));

        let (change, _) = approval_request(
            "item/fileChange/requestApproval",
            &json!({"threadId": "t", "turnId": "u", "itemId": "i7", "startedAtMs": 1}),
            &files,
        )
        .unwrap();
        assert_eq!(change.kind, PermissionKind::FileChange);
        assert_eq!(change.input["files"], json!(["C:\\w\\a.txt"]));

        let (legacy, format) = approval_request(
            "execCommandApproval",
            &json!({"command": ["git", "push"], "cwd": "C:\\w"}),
            &files,
        )
        .unwrap();
        assert_eq!(format, ApprovalFormat::Legacy);
        assert_eq!(legacy.input["command"], "git push");
        assert!(approval_request("item/tool/requestUserInput", &json!({}), &files).is_none());

        let answer = |f, d: Decision| approval_answer(f, &d)["decision"].clone();
        assert_eq!(answer(ApprovalFormat::V2, Decision::Allow), "accept");
        assert_eq!(
            answer(ApprovalFormat::V2, Decision::AllowAlways),
            "acceptForSession"
        );
        assert_eq!(
            answer(ApprovalFormat::V2, Decision::Deny { reason: None }),
            "decline"
        );
        assert_eq!(answer(ApprovalFormat::V2, Decision::Cancelled), "cancel");
        assert_eq!(answer(ApprovalFormat::Legacy, Decision::Allow), "approved");
        assert_eq!(
            answer(ApprovalFormat::Legacy, Decision::Deny { reason: None }),
            "denied"
        );
    }

    #[test]
    fn refused_commands_and_file_changes_say_why() {
        let events: Vec<AgentEvent> = [
            ("item/completed", json!({"item": {"type": "commandExecution", "id": "1",
                "command": "rm x", "status": "declined", "exitCode": null, "aggregatedOutput": null}})),
            ("item/completed", json!({"item": {"type": "commandExecution", "id": "2",
                "command": "false", "status": "failed", "exitCode": 1, "aggregatedOutput": null}})),
            ("item/started", json!({"item": {"type": "fileChange", "id": "3", "status": "inProgress",
                "changes": [{"path": "a.txt", "kind": {"type": "add"}, "diff": ""}]}})),
            ("item/completed", json!({"item": {"type": "fileChange", "id": "3", "status": "declined", "changes": []}})),
        ]
        .iter()
        .flat_map(|(m, p)| notification(m, p))
        .collect();
        assert_eq!(
            events,
            vec![
                AgentEvent::ToolResult {
                    name: "shell".into(),
                    ok: false,
                    summary: "Declined: the command was not approved".into(),
                    output: None
                },
                AgentEvent::ToolResult {
                    name: "shell".into(),
                    ok: false,
                    summary: "Exited with code 1".into(),
                    output: None
                },
                AgentEvent::ToolCall {
                    name: "file_change".into(),
                    args: json!({"files": ["a.txt"]})
                },
                AgentEvent::ToolResult {
                    name: "file_change".into(),
                    ok: false,
                    summary: "Declined: the change was not approved".into(),
                    output: None
                },
            ]
        );
    }

    #[test]
    fn exec_fallback_events() {
        let mut thread = None;
        let lines = [
            json!({"type": "thread.started", "thread_id": "th-1"}),
            json!({"type": "item.started", "item": {"id": "1", "type": "mcp_tool_call", "server": "browsermcp", "tool": "system_ping", "arguments": {}, "status": "in_progress"}}),
            json!({"type": "item.completed", "item": {"id": "1", "type": "mcp_tool_call", "tool": "system_ping", "status": "completed", "result": {"content": [{"type": "text", "text": "{}"}]}}}),
            json!({"type": "item.completed", "item": {"id": "2", "type": "agent_message", "text": "ok"}}),
            json!({"type": "turn.completed", "usage": {}}),
        ];
        let events: Vec<_> = lines
            .iter()
            .flat_map(|l| exec_event(l, &mut thread))
            .collect();
        assert_eq!(thread.as_deref(), Some("th-1"));
        assert_eq!(events.len(), 5);
        assert_eq!(events[3], AgentEvent::Turn("exec".into()));
        assert_eq!(events[4], AgentEvent::Done("end_turn".into()));
    }

    #[test]
    fn exec_args_read_the_prompt_from_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let access = access(dir.path());
        let args = exec_args(None, "http://127.0.0.1:1/mcp", Some("gpt-x"), &access);
        assert_eq!(args.first().map(String::as_str), Some("exec"));
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(args.windows(2).any(|w| w == ["-m", "gpt-x"]));
        assert!(args.contains(&"sandbox_mode=\"workspace-write\"".to_string()));
        let args = exec_args(Some("th-1"), "http://127.0.0.1:1/mcp", None, &access);
        assert_eq!(&args[..3], ["exec", "resume", "th-1"]);
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    /// Regression for the `codex exec` fallback: a fake `codex.cmd` floods stderr far past the
    /// pipe buffer (blocks forever unless we drain it), then reads the prompt from stdin.
    /// A prompt that looks like flags must arrive as text, not as arguments.
    #[cfg(windows)]
    #[tokio::test]
    async fn exec_mode_drains_stderr_and_sends_prompt_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex.cmd");
        let script = [
            "@echo off",
            ">\"%~dp0args.txt\" echo(%*",
            "for /L %%i in (1,1,3000) do echo stderr noise to fill the pipe buffer quickly 1>&2",
            "set /p PROMPT=",
            ">\"%~dp0prompt.txt\" echo(%PROMPT%",
            r#"echo {"type":"item.completed","item":{"id":"2","type":"agent_message","text":"ok"}}"#,
            r#"echo {"type":"turn.completed","usage":{}}"#,
        ]
        .join("\r\n");
        std::fs::write(&fake, script).unwrap();

        let (session, _) = Session::new(
            "codex-test".into(),
            "codex".into(),
            None,
            "#888888".into(),
            bmcp_core::EventBus::new(),
            None,
        );
        let codex = Codex {
            session: Arc::new(session),
            binary: fake,
            model: None,
            mcp_url: "http://127.0.0.1:9/mcp?agent=codex-test".into(),
            access: access(dir.path()),
            resume: None,
            rollback: None,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(Input::text("--help -m gpt-x hi")).unwrap();
        drop(tx); // one message, then the queue ends and the session stops
        tokio::time::timeout(Duration::from_secs(60), codex.exec_mode(rx))
            .await
            .expect("codex exec must not hang on a full stderr pipe");

        let prompt = std::fs::read_to_string(dir.path().join("prompt.txt")).unwrap();
        assert_eq!(prompt.trim_end(), "--help -m gpt-x hi");
        let args = std::fs::read_to_string(dir.path().join("args.txt")).unwrap();
        assert!(args.trim_end().ends_with(" -"), "args: {args}");
        assert!(!args.contains("--help"), "args: {args}");
        let transcript = serde_json::to_string(&codex.session.transcript()).unwrap();
        assert!(transcript.contains(r#""text":"ok""#), "{transcript}");
        assert_eq!(codex.session.status(), Status::Stopped);
    }

    /// Manual check against the installed Codex: handshake + thread/start only (no model call).
    /// `cargo test -p bmcp-ai codex_handshake -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs a local Codex install"]
    async fn codex_handshake() {
        let binary = crate::cli::find_codex(None).expect("codex not found");
        let dir = tempfile::tempdir().unwrap();
        let mut child = command(&binary, dir.path())
            .arg("app-server")
            .args(["-c", &mcp_override("http://127.0.0.1:9/mcp?agent=test")])
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut send = async |v: Value| stdin.write_all(format!("{v}\n").as_bytes()).await.unwrap();
        send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "browsermcp-test", "version": "0"}}})).await;
        send(json!({"method": "initialized"})).await;
        let mut params = thread_start_params(None, "test", &access(dir.path()));
        params["ephemeral"] = json!(true); // keep the user's Codex history clean
        send(json!({"id": 2, "method": "thread/start", "params": params})).await;
        let mut thread = None;
        while thread.is_none() {
            let line = tokio::time::timeout(Duration::from_secs(60), lines.next_line())
                .await
                .expect("timeout")
                .unwrap()
                .expect("codex closed stdout");
            println!("<- {line}");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == 2 {
                assert!(v.get("error").is_none(), "thread/start failed: {v}");
                thread = v["result"]["thread"]["id"].as_str().map(str::to_string);
            }
        }
        println!("thread id: {thread:?}");
        child.kill().await.unwrap();
    }
}
