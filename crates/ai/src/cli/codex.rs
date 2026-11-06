//! Codex as an agent.
//!
//! Preferred: `codex app-server` — JSON-RPC over stdio, one JSON object per line, without the
//! `"jsonrpc"` field (protocol from `codex app-server generate-json-schema`, codex-cli 0.166):
//!
//! 1. `initialize {clientInfo}` → result; then the `initialized` notification;
//! 2. `thread/start {cwd, approvalPolicy, sandbox, model?, developerInstructions}` → `{thread: {id}}`;
//! 3. per user message `turn/start {threadId, input: [{type: "text", text}]}` → `{turn: {id}}`;
//!    notifications `item/agentMessage/delta`, `item/started`, `item/completed`
//!    (`agentMessage`, `mcpToolCall`, `commandExecution`), `turn/completed {turn: {status}}`;
//! 4. stop: `turn/interrupt {threadId, turnId}`, then kill.
//!
//! The browser's MCP server is passed as a config override:
//! `-c mcp_servers.browsermcp.url="http://127.0.0.1:47800/mcp?agent=<id>"`. Approvals are off
//! (`approvalPolicy: never`) and the shell sandbox is read-only: Codex drives the browser through
//! MCP, not through shell commands.
//!
//! Fallback when the app-server does not start (old Codex): `codex exec --json "<prompt>"` per
//! message (`codex exec resume <thread> --json …` for the next ones). That path is untested
//! against a real Codex.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot};

use super::{StderrTail, command};
use crate::session::{AgentEvent, Session, Status, summarize};

const SERVER: &str = "browsermcp";
/// How long a JSON-RPC request may take (thread/start loads MCP servers).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Codex {
    pub session: Arc<Session>,
    pub binary: PathBuf,
    pub model: Option<String>,
    pub mcp_url: String,
    pub cwd: PathBuf,
}

/// `-c` override that adds the browser's MCP server.
pub fn mcp_override(url: &str) -> String {
    format!("mcp_servers.{SERVER}.url=\"{url}\"")
}

/// `thread/start` parameters.
pub fn thread_start_params(cwd: &str, model: Option<&str>, instructions: &str) -> Value {
    let mut params = json!({
        "cwd": cwd,
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "developerInstructions": instructions,
    });
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        params["model"] = json!(model);
    }
    params
}

/// `turn/start` parameters.
pub fn turn_start_params(thread: &str, text: &str) -> Value {
    json!({ "threadId": thread, "input": [{ "type": "text", "text": text, "text_elements": [] }] })
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
            "commandExecution" => vec![AgentEvent::ToolResult {
                name: "shell".into(),
                ok: item["exitCode"] == 0,
                summary: summarize(&item["aggregatedOutput"]),
                output: None,
            }],
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
            vec![AgentEvent::Done(reason)]
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

/// Answer for a request the server sends us. Approvals are declined (`approvalPolicy: never`
/// should prevent them); MCP elicitations (tool-call confirmations) for our server are accepted.
pub fn server_request_answer(method: &str) -> Result<Value, Value> {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Ok(json!({ "decision": "decline" }))
        }
        "execCommandApproval" | "applyPatchApproval" => Ok(json!({ "decision": "denied" })),
        "mcpServer/elicitation/request" => Ok(json!({ "action": "accept", "content": {} })),
        _ => Err(
            json!({ "code": -32601, "message": format!("{method} is not supported by BrowserMCP") }),
        ),
    }
}

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
        "turn.completed" => vec![AgentEvent::Done("end_turn".into())],
        "turn.failed" => vec![AgentEvent::Done(format!(
            "error: {}",
            v["error"]["message"].as_str().unwrap_or("turn failed")
        ))],
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
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        self.write(&json!({ "id": id, "method": method, "params": params }))
            .await
            .map_err(|e| format!("{method}: cannot write to codex: {e}"))?;
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Err(_) => Err(format!("{method}: no answer from codex")),
            Ok(Err(_)) => Err(format!("{method}: codex exited")),
            Ok(Ok(Err(error))) => Err(format!(
                "{method}: {}",
                error["message"].as_str().unwrap_or("error")
            )),
            Ok(Ok(Ok(result))) => Ok(result),
        }
    }

    async fn notify(&self, method: &str) -> std::io::Result<()> {
        self.write(&json!({ "method": method })).await
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Codex {
    fn spawn_app_server(&self) -> std::io::Result<Child> {
        let mut cmd = command(&self.binary, &self.cwd);
        cmd.arg("app-server")
            .args(["-c", &mcp_override(&self.mcp_url)]);
        cmd.spawn()
    }

    /// The session task.
    pub async fn run(self, input: mpsc::UnboundedReceiver<String>) {
        let session = self.session.clone();
        match self.spawn_app_server() {
            Ok(child) => match self.app_server(child, input).await {
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
        mut input: mpsc::UnboundedReceiver<String>,
    ) -> Result<(), (String, Option<mpsc::UnboundedReceiver<String>>)> {
        let session = self.session.clone();
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

        // stdout: responses → pending requests; notifications → events; server requests → answers.
        let reader = {
            let (rpc, session, turn) = (rpc.clone(), session.clone(), turn.clone());
            let stdout = BufReader::new(stdout);
            tokio::spawn(async move {
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
                            for event in notification(method, &msg["params"]) {
                                let done = matches!(event, AgentEvent::Done(_));
                                session.apply(event);
                                if done {
                                    *lock(&turn) = None;
                                    session.set_status(Status::Idle);
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

        // Handshake.
        let started = async {
            rpc.request(
                "initialize",
                json!({ "clientInfo": { "name": "browsermcp", "title": "BrowserMCP", "version": env!("CARGO_PKG_VERSION") } }),
            )
            .await?;
            rpc.notify("initialized").await.map_err(|e| e.to_string())?;
            let params = thread_start_params(
                &self.cwd.to_string_lossy(),
                self.model.as_deref(),
                &crate::agent::system_prompt(&session.id),
            );
            let result = rpc.request("thread/start", params).await?;
            result["thread"]["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("thread/start returned no thread id: {result}"))
        };
        let thread = tokio::select! {
            _ = session.cancel.cancelled() => {
                let _ = child.kill().await;
                session.apply(AgentEvent::Done("stopped".into()));
                session.set_status(Status::Stopped);
                return Ok(());
            }
            thread = started => match thread {
                Ok(thread) => thread,
                Err(err) => {
                    let _ = child.kill().await;
                    let tail = stderr.text();
                    return Err((format!("{err} {tail}").trim().to_string(), Some(input)));
                }
            },
        };

        // Turns.
        let exit = loop {
            tokio::select! {
                _ = session.cancel.cancelled() => break None,
                status = child.wait() => break Some(status),
                text = input.recv() => {
                    let Some(text) = text else { break None };
                    session.set_status(Status::Running);
                    if let Err(err) = rpc.request("turn/start", turn_start_params(&thread, &text)).await {
                        session.apply(AgentEvent::Notice(err.clone()));
                        session.apply(AgentEvent::Done(format!("error: {err}")));
                        session.set_status(Status::Idle);
                    }
                }
            }
        };

        match exit {
            None => {
                let running = lock(&turn).clone();
                if let Some(turn_id) = running {
                    let interrupt = rpc.request(
                        "turn/interrupt",
                        json!({ "threadId": thread, "turnId": turn_id }),
                    );
                    let _ = tokio::time::timeout(Duration::from_secs(3), interrupt).await;
                }
                if session.status() == Status::Running {
                    session.apply(AgentEvent::Done("stopped".into()));
                }
                let _ = child.kill().await;
                reader.abort();
                session.set_status(Status::Stopped);
                Ok(())
            }
            Some(status) => {
                let _ = reader.await;
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
    async fn exec_mode(&self, mut input: mpsc::UnboundedReceiver<String>) {
        let session = self.session.clone();
        let mut thread: Option<String> = None;
        loop {
            let text = tokio::select! {
                _ = session.cancel.cancelled() => break,
                text = input.recv() => match text { Some(t) => t, None => break },
            };
            session.set_status(Status::Running);
            let mut cmd = command(&self.binary, &self.cwd);
            cmd.arg("exec");
            if let Some(thread) = &thread {
                cmd.args(["resume", thread]);
            }
            cmd.args([
                "--json",
                "--skip-git-repo-check",
                "-c",
                &mcp_override(&self.mcp_url),
            ]);
            if let Some(model) = self.model.as_deref().filter(|m| !m.is_empty()) {
                cmd.args(["-m", model]);
            }
            cmd.arg(&text).stdin(std::process::Stdio::null());
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => {
                    session.apply(AgentEvent::Done(format!(
                        "error: cannot start codex exec: {err}"
                    )));
                    session.set_status(Status::Failed);
                    return;
                }
            };
            let Some(stdout) = child.stdout.take() else {
                break;
            };
            let mut lines = BufReader::new(stdout).lines();
            let mut done = false;
            loop {
                tokio::select! {
                    _ = session.cancel.cancelled() => {
                        let _ = child.kill().await;
                        session.apply(AgentEvent::Done("stopped".into()));
                        session.set_status(Status::Stopped);
                        return;
                    }
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                        for event in exec_event(&v, &mut thread) {
                            done |= matches!(event, AgentEvent::Done(_));
                            session.apply(event);
                        }
                    }
                }
            }
            let _ = child.wait().await;
            if !done {
                session.apply(AgentEvent::Done("end_turn".into()));
            }
            session.set_status(Status::Idle);
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
                AgentEvent::Done("end_turn".into()),
                AgentEvent::Done("error: quota".into()),
            ]
        );
    }

    #[test]
    fn requests_and_params() {
        assert_eq!(
            server_request_answer("item/commandExecution/requestApproval").unwrap()["decision"],
            "decline"
        );
        assert!(server_request_answer("item/tool/requestUserInput").is_err());
        assert_eq!(
            mcp_override("http://127.0.0.1:1/mcp?agent=codex-1"),
            "mcp_servers.browsermcp.url=\"http://127.0.0.1:1/mcp?agent=codex-1\""
        );
        let p = thread_start_params("C:\\w", Some("gpt-x"), "hi");
        assert_eq!(p["approvalPolicy"], "never");
        assert_eq!(p["model"], "gpt-x");
        assert_eq!(turn_start_params("t1", "go")["input"][0]["text"], "go");
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
        assert_eq!(events.len(), 4);
        assert_eq!(events[3], AgentEvent::Done("end_turn".into()));
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
        let mut params = thread_start_params(&dir.path().to_string_lossy(), None, "test");
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
