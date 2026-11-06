//! Claude Code as an agent: `claude -p --input-format stream-json --output-format stream-json`.
//!
//! * Input (stdin), one JSON object per line:
//!   `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"…"}]}}`.
//!   The process stays alive between messages until stdin closes or it is killed.
//! * Output (stdout), one JSON object per line: `system` (`init` lists MCP servers),
//!   `stream_event` (partial text, with `--include-partial-messages`), `assistant` (text and
//!   `tool_use` blocks), `user` (`tool_result` blocks), `result` (end of one answer).
//! * Tools: only the browser's MCP server (`--mcp-config <file> --strict-mcp-config`), allowed
//!   without prompts (`--allowedTools mcp__browsermcp`); anything else is refused
//!   (`--permission-mode dontAsk`), since nobody can answer a permission prompt here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

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
    pub cwd: PathBuf,
}

/// The MCP config file content.
pub fn mcp_config(url: &str) -> Value {
    json!({ "mcpServers": { SERVER: { "type": "http", "url": url } } })
}

/// Command-line arguments (without the binary).
pub fn args(mcp_config: &str, model: Option<&str>, append_prompt: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--strict-mcp-config",
        "--permission-mode",
        "dontAsk",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // `--mcp-config` and `--allowedTools` take several values: each is followed by a flag.
    args.extend(["--mcp-config".into(), mcp_config.into()]);
    args.extend(["--allowedTools".into(), format!("mcp__{SERVER}")]);
    args.extend(["--append-system-prompt".into(), append_prompt.into()]);
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), model.into()]);
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

/// Turns stream-json output lines into [`AgentEvent`]s.
#[derive(Debug, Default)]
pub struct Parser {
    /// tool_use id → tool name, to name the matching tool_result.
    tools: HashMap<String, String>,
}

impl Parser {
    pub fn line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new(); // not JSON (should not happen); ignore
        };
        match v["type"].as_str().unwrap_or_default() {
            "system" if v["subtype"] == "init" => init_notices(&v),
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

impl ClaudeCode {
    /// The session task: run the CLI until stopped or it exits.
    pub async fn run(self, mut input: mpsc::UnboundedReceiver<String>) {
        let session = self.session.clone();
        let fail = |message: String| {
            session.apply(AgentEvent::Notice(message.clone()));
            session.apply(AgentEvent::Done(format!("error: {message}")));
            session.set_status(Status::Failed);
        };

        let config = mcp_config(&self.mcp_url).to_string();
        if let Err(err) = std::fs::write(&self.mcp_config, config) {
            return fail(format!("cannot write {}: {err}", self.mcp_config.display()));
        }
        let mut cmd = command(&self.binary, &self.cwd);
        cmd.args(args(
            &self.mcp_config.to_string_lossy(),
            self.model.as_deref(),
            &crate::agent::system_prompt(&session.id),
        ));
        // `tree` also covers the real agent behind an npm `claude.cmd` shim (a grandchild).
        let (mut child, tree) = match spawn(&mut cmd) {
            Ok(spawned) => spawned,
            Err(err) => return fail(format!("cannot start {}: {err}", self.binary.display())),
        };
        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return fail("child process has no stdio".into());
        };
        let stderr = StderrTail::collect(stderr, "claude");

        // stdout → events.
        let reader = {
            let session = session.clone();
            tokio::spawn(async move {
                let mut parser = Parser::default();
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    for event in parser.line(&line) {
                        let done = matches!(event, AgentEvent::Done(_));
                        session.apply(event);
                        if done {
                            session.set_status(Status::Idle);
                        }
                    }
                }
            })
        };

        // Relay user messages until stopped or the process ends.
        let exit = loop {
            tokio::select! {
                _ = session.cancel.cancelled() => break None,
                status = child.wait() => break Some(status),
                text = input.recv() => {
                    let Some(text) = text else { break None };
                    session.set_status(Status::Running);
                    let written = async {
                        stdin.write_all(user_line(&text).as_bytes()).await?;
                        stdin.flush().await
                    }.await;
                    if let Err(err) = written {
                        session.apply(AgentEvent::Notice(format!("cannot write to Claude Code: {err}")));
                    }
                }
            }
        };

        match exit {
            None => {
                let was_running = session.status() == Status::Running;
                // Close stdin first (claude exits on EOF), then kill the whole tree: killing
                // only `cmd.exe` would leave node.exe running and holding stdout open.
                drop(stdin);
                kill_tree(&mut child, &tree).await;
                join_reader(reader).await;
                if was_running {
                    session.apply(AgentEvent::Done("stopped".into()));
                }
                session.set_status(Status::Stopped);
            }
            Some(status) => {
                join_reader(reader).await;
                tree.kill(); // helpers the agent left behind
                let ok = status.as_ref().is_ok_and(|s| s.success());
                if ok {
                    session.set_status(Status::Stopped);
                } else {
                    let code = status
                        .map(|s| s.to_string())
                        .unwrap_or_else(|e| e.to_string());
                    fail(format!("Claude Code exited ({code}): {}", stderr.text()));
                }
            }
        }
        let _ = std::fs::remove_file(&self.mcp_config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn parses_recorded_stream_json() {
        let mut parser = Parser::default();
        let events: Vec<AgentEvent> = SAMPLE.iter().flat_map(|l| parser.line(l)).collect();
        assert_eq!(
            events,
            vec![
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
        std::fs::write(&shim, "@ping -n 60 127.0.0.1\r\n").unwrap();
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
        let agent = ClaudeCode {
            session: session.clone(),
            binary: shim,
            model: None,
            mcp_url: "http://127.0.0.1:9/mcp?agent=cc-test".into(),
            mcp_config: mcp_config.clone(),
            cwd: dir.path().to_path_buf(),
        };
        let task = tokio::spawn(agent.run(input));
        // Give cmd.exe time to start ping.exe.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(mcp_config.exists());

        let stopped = Instant::now();
        session.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the session task must finish after stop")
            .unwrap();
        // Quicker than the reader grace period: stdout closed because ping.exe died too.
        assert!(
            stopped.elapsed() < Duration::from_millis(2500),
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

        let args = args("C:\\d\\s.mcp.json", Some("sonnet"), "prompt");
        let joined = args.join(" ");
        for flag in [
            "-p",
            "--input-format stream-json",
            "--output-format stream-json",
            "--verbose",
            "--strict-mcp-config",
            "--mcp-config C:\\d\\s.mcp.json",
            "--allowedTools mcp__browsermcp",
            "--model sonnet",
        ] {
            assert!(joined.contains(flag), "missing {flag} in {joined}");
        }
        assert_eq!(
            mcp_config("http://127.0.0.1:1/mcp?agent=a")["mcpServers"]["browsermcp"]["url"],
            "http://127.0.0.1:1/mcp?agent=a"
        );
    }
}
