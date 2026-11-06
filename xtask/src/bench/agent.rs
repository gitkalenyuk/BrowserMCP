//! Running one benchmark task through a CLI agent, headless, and reading what it did.
//!
//! * Claude Code: `claude -p <prompt> --output-format stream-json --verbose --mcp-config <file>
//!   --strict-mcp-config --allowedTools "mcp__browsermcp__*"`. Tool calls are the `tool_use`
//!   blocks of `assistant` events; tokens come from the final `result` event's `usage`.
//! * Codex: `codex exec --json -c mcp_servers.browsermcp.url="<url>" -` (prompt on stdin).
//!   Tool calls are `item.completed` items of type `mcp_tool_call`; tokens come from
//!   `turn.completed.usage`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Result;

/// Which CLI agent drives the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

impl AgentKind {
    pub fn parse(name: &str) -> Result<AgentKind> {
        match name {
            "claude-code" | "claude" => Ok(AgentKind::ClaudeCode),
            "codex" => Ok(AgentKind::Codex),
            other => Err(format!("unknown agent '{other}' (claude-code | codex)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
        }
    }
}

/// Everything we learn from one agent run.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AgentRun {
    /// Calls per tool (without the `mcp__browsermcp__` prefix).
    pub tools: BTreeMap<String, u32>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: Option<f64>,
    pub turns: Option<u64>,
    pub model: Option<String>,
    /// MCP server status from Claude Code's init event (`connected`, `failed`…).
    pub mcp_status: Option<String>,
    /// The agent's final answer.
    pub final_text: String,
    /// Set when the agent itself failed (error result, timeout, could not start).
    pub error: Option<String>,
    pub wall_ms: u64,
    /// The raw event stream (saved to `target/bench-work/<task>.jsonl` for debugging).
    pub events: Vec<Value>,
}

impl AgentRun {
    pub fn tool_calls(&self) -> u32 {
        self.tools.values().sum()
    }

    /// Every token the model read or wrote (fresh input + cache reads/writes + output).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens + self.output_tokens
    }

    pub fn to_json(&self) -> Value {
        json!({
            "tool_calls": self.tool_calls(),
            "tools": self.tools,
            "tokens": {
                "input": self.input_tokens,
                "output": self.output_tokens,
                "cache_read": self.cache_read_tokens,
                "cache_creation": self.cache_creation_tokens,
                "total": self.total_tokens(),
            },
            "cost_usd": self.cost_usd,
            "turns": self.turns,
            "model": self.model,
            "mcp_status": self.mcp_status,
            "final_text": self.final_text,
            "agent_error": self.error,
            "wall_ms": self.wall_ms,
        })
    }
}

/// Options shared by all tasks of a run.
pub struct AgentOptions {
    pub kind: AgentKind,
    /// `http://127.0.0.1:<port>/mcp?agent=bench`
    pub mcp_url: String,
    /// Empty working folder (so no project settings / CLAUDE.md leak into the run).
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub timeout: Duration,
}

/// Run one prompt to the end (or the timeout).
pub fn run(opts: &AgentOptions, prompt: &str) -> AgentRun {
    let started = Instant::now();
    let mut result = match opts.kind {
        AgentKind::ClaudeCode => run_claude(opts, prompt),
        AgentKind::Codex => run_codex(opts, prompt),
    }
    .unwrap_or_else(|err| AgentRun {
        error: Some(err),
        ..AgentRun::default()
    });
    result.wall_ms = started.elapsed().as_millis() as u64;
    result
}

/// A child process with the host agent's environment removed: we may run inside a Claude
/// Code session, and the child must be a plain, top-level CLI using the user's own login.
fn command(program: &str, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, _) in std::env::vars() {
        if key.starts_with("CLAUDE") || key == "ANTHROPIC_BASE_URL" {
            cmd.env_remove(key);
        }
    }
    cmd
}

fn run_claude(opts: &AgentOptions, prompt: &str) -> Result<AgentRun> {
    let config = opts.cwd.join("bench.mcp.json");
    let server = json!({ "mcpServers": { "browsermcp": { "type": "http", "url": opts.mcp_url } } });
    std::fs::write(&config, server.to_string()).map_err(|e| e.to_string())?;
    let mut cmd = command("claude", &opts.cwd);
    cmd.args(["-p", prompt, "--output-format", "stream-json", "--verbose"])
        .args(["--strict-mcp-config", "--mcp-config"])
        .arg(&config)
        .args(["--allowedTools", "mcp__browsermcp__*"])
        // The benchmark is "without screenshots".
        .args(["--disallowedTools", "mcp__browsermcp__page_screenshot"])
        // Anything not allowed is refused instead of waiting for an approval nobody gives.
        .args(["--permission-mode", "dontAsk"]);
    if let Some(model) = &opts.model {
        cmd.args(["--model", model]);
    }
    let lines = run_lines(cmd, None, opts.timeout)?;
    let mut run = parse_claude(&lines.lines);
    run.events = lines.lines;
    if lines.timed_out {
        run.error = Some(format!("timed out after {:?}", opts.timeout));
    } else if run.error.is_none() && run.final_text.is_empty() && run.tool_calls() == 0 {
        run.error = Some(format!("no result; stderr: {}", tail(&lines.stderr, 400)));
    }
    Ok(run)
}

fn run_codex(opts: &AgentOptions, prompt: &str) -> Result<AgentRun> {
    let program = if cfg!(windows) && which("codex.cmd") {
        "codex.cmd"
    } else {
        "codex"
    };
    let mut cmd = command(program, &opts.cwd);
    cmd.args(["exec", "--json", "--skip-git-repo-check", "-c"])
        .arg(format!("mcp_servers.browsermcp.url=\"{}\"", opts.mcp_url));
    if let Some(model) = &opts.model {
        cmd.args(["-m", model]);
    }
    cmd.arg("-"); // prompt from stdin: a prompt that looks like flags stays text
    let lines = run_lines(cmd, Some(prompt), opts.timeout)?;
    let mut run = parse_codex(&lines.lines);
    run.events = lines.lines;
    if lines.timed_out {
        run.error = Some(format!("timed out after {:?}", opts.timeout));
    } else if run.error.is_none() && run.final_text.is_empty() && run.tool_calls() == 0 {
        run.error = Some(format!("no result; stderr: {}", tail(&lines.stderr, 400)));
    }
    Ok(run)
}

fn which(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

struct Lines {
    lines: Vec<Value>,
    stderr: String,
    timed_out: bool,
}

/// Start `cmd`, optionally write `stdin`, collect stdout JSON lines until it exits or
/// `timeout` passes (then kill it — only this child, by its own handle).
fn run_lines(mut cmd: Command, stdin: Option<&str>, timeout: Duration) -> Result<Lines> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {:?}: {e}", cmd.get_program()))?;
    let mut input = child.stdin.take();
    if let (Some(pipe), Some(text)) = (input.as_mut(), stdin) {
        pipe.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    drop(input); // EOF: Claude Code reads nothing, Codex reads the prompt
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(value);
            }
        }
    });
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            text.push_str(&line);
            text.push('\n');
        }
        text
    });

    let deadline = Instant::now() + timeout;
    let mut lines = Vec::new();
    let mut timed_out = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(value) => lines.push(value),
            Err(mpsc::RecvTimeoutError::Disconnected) => break, // stdout closed: exited
            Err(mpsc::RecvTimeoutError::Timeout) if left.is_zero() => {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = child.wait();
    let stderr = if timed_out {
        String::new()
    } else {
        err_reader.join().unwrap_or_default()
    };
    Ok(Lines {
        lines,
        stderr,
        timed_out,
    })
}

fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    let start = text.len().saturating_sub(max);
    // Do not cut inside a UTF-8 character.
    let start = (start..=text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    text[start..].to_owned()
}

/// Tool name as the browser knows it: `mcp__browsermcp__nav_goto` → `nav_goto`.
fn short_tool(name: &str) -> String {
    name.strip_prefix("mcp__browsermcp__")
        .unwrap_or(name)
        .to_owned()
}

/// Read Claude Code's `--output-format stream-json` events.
pub fn parse_claude(lines: &[Value]) -> AgentRun {
    let mut run = AgentRun::default();
    let mut seen = std::collections::HashSet::new();
    for event in lines {
        match event["type"].as_str().unwrap_or_default() {
            "system" if event["subtype"] == "init" => {
                run.model = event["model"].as_str().map(str::to_owned);
                run.mcp_status = event["mcp_servers"]
                    .as_array()
                    .and_then(|servers| servers.iter().find(|s| s["name"] == "browsermcp"))
                    .and_then(|s| s["status"].as_str())
                    .map(str::to_owned);
            }
            "assistant" => {
                let blocks = event["message"]["content"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for block in blocks {
                    // The same message can be streamed in several events: count each id once.
                    let id = block["id"].as_str().unwrap_or_default().to_owned();
                    if block["type"] == "tool_use" && seen.insert(id) {
                        let name = short_tool(block["name"].as_str().unwrap_or("?"));
                        *run.tools.entry(name).or_default() += 1;
                    }
                }
            }
            "result" => {
                let usage = &event["usage"];
                run.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                run.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                run.cache_read_tokens = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                run.cache_creation_tokens =
                    usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                run.cost_usd = event["total_cost_usd"].as_f64();
                run.turns = event["num_turns"].as_u64();
                run.final_text = event["result"].as_str().unwrap_or_default().to_owned();
                if event["is_error"] == true || event["subtype"] != "success" {
                    run.error = Some(format!(
                        "{}: {}",
                        event["subtype"].as_str().unwrap_or("error"),
                        crate::bench::cut(&run.final_text, 200)
                    ));
                }
            }
            _ => {}
        }
    }
    run
}

/// Read `codex exec --json` events.
pub fn parse_codex(lines: &[Value]) -> AgentRun {
    let mut run = AgentRun::default();
    for event in lines {
        let item = &event["item"];
        match event["type"].as_str().unwrap_or_default() {
            "item.completed" if item["type"] == "mcp_tool_call" => {
                let name = item["tool"].as_str().unwrap_or("?").to_owned();
                *run.tools.entry(name).or_default() += 1;
            }
            "item.completed" if item["type"] == "agent_message" => {
                run.final_text = item["text"].as_str().unwrap_or_default().to_owned();
            }
            "turn.completed" => {
                let usage = &event["usage"];
                // Codex reports cached input as a part of input_tokens.
                let input = usage["input_tokens"].as_u64().unwrap_or(0);
                let cached = usage["cached_input_tokens"].as_u64().unwrap_or(0);
                run.input_tokens += input.saturating_sub(cached);
                run.cache_read_tokens += cached;
                run.output_tokens += usage["output_tokens"].as_u64().unwrap_or(0);
                run.turns = Some(run.turns.unwrap_or(0) + 1);
            }
            "turn.failed" | "error" => {
                let message = event["error"]["message"]
                    .as_str()
                    .or(event["message"].as_str())
                    .unwrap_or("error");
                run.error = Some(message.to_owned());
            }
            _ => {}
        }
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream_counts_tools_once_and_reads_usage() {
        let lines = vec![
            json!({"type": "system", "subtype": "init", "model": "claude-x",
                   "mcp_servers": [{"name": "browsermcp", "status": "connected"}]}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "Let me look."},
                {"type": "tool_use", "id": "t1", "name": "mcp__browsermcp__nav_goto", "input": {}}]}}),
            // The same block again (streamed in two events).
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "mcp__browsermcp__nav_goto", "input": {}}]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "t2", "name": "mcp__browsermcp__page_act", "input": {}},
                {"type": "tool_use", "id": "t3", "name": "mcp__browsermcp__page_act", "input": {}}]}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 4,
                   "result": "DONE", "total_cost_usd": 0.05,
                   "usage": {"input_tokens": 10, "output_tokens": 200,
                             "cache_read_input_tokens": 3000, "cache_creation_input_tokens": 500}}),
        ];
        let run = parse_claude(&lines);
        assert_eq!(run.tool_calls(), 3);
        assert_eq!(run.tools["nav_goto"], 1);
        assert_eq!(run.tools["page_act"], 2);
        assert_eq!(run.total_tokens(), 3710);
        assert_eq!(run.model.as_deref(), Some("claude-x"));
        assert_eq!(run.mcp_status.as_deref(), Some("connected"));
        assert_eq!(run.final_text, "DONE");
        assert_eq!(run.error, None);

        let failed = parse_claude(&[json!({"type": "result", "subtype": "error_max_turns",
                                           "is_error": true, "usage": {}})]);
        assert!(failed.error.unwrap().starts_with("error_max_turns"));
    }

    #[test]
    fn codex_stream_counts_mcp_calls_and_usage() {
        let lines = vec![
            json!({"type": "item.completed", "item": {"type": "mcp_tool_call", "tool": "nav_goto"}}),
            json!({"type": "item.completed", "item": {"type": "mcp_tool_call", "tool": "page_act"}}),
            json!({"type": "item.completed", "item": {"type": "agent_message", "text": "done"}}),
            json!({"type": "turn.completed", "usage": {"input_tokens": 1000, "cached_input_tokens": 800, "output_tokens": 50}}),
        ];
        let run = parse_codex(&lines);
        assert_eq!(run.tool_calls(), 2);
        assert_eq!(
            (run.input_tokens, run.cache_read_tokens, run.output_tokens),
            (200, 800, 50)
        );
        assert_eq!(run.final_text, "done");
    }

    #[test]
    fn agent_names() {
        assert_eq!(
            AgentKind::parse("claude-code").unwrap(),
            AgentKind::ClaudeCode
        );
        assert_eq!(AgentKind::parse("codex").unwrap().name(), "codex");
        assert!(AgentKind::parse("gpt").is_err());
        assert_eq!(tail("abcdef", 3), "def");
    }
}
