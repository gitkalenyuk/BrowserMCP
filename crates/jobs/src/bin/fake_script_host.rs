//! `bmcp-fake-script-host`: a stand-in for `bmcp-script-host` in the tests (no V8, starts in
//! milliseconds). Same command line:
//!
//! ```text
//! bmcp-fake-script-host run <file> [--ws <url>] [--input k=v]... [--inputs-file <json>]
//!                       [--timeout-secs N]
//! ```
//!
//! The "script" is a list of lines, run top to bottom. `{{key}}` is replaced by the input
//! `key`. A line may start with conditions `when key=value` (repeatable). Commands:
//!
//! | Line | Effect |
//! |---|---|
//! | `out <text>` / `err <text>` | print a line to stdout / stderr |
//! | `result <json>` | print `##bmcp:result <json>` |
//! | `time <name>` | `result {"<name>": <ms since epoch>}` |
//! | `args` | `result {"ws": …, "inputs": {…}, "timeout_secs": …, "tab_defaults": BMCP_TAB_DEFAULTS}` |
//! | `sleep <ms>` | sleep; past `--timeout-secs` it exits 124 like the real host |
//! | `fail <CODE> <command> <message>` | uncaught failed browser call, exit 1 |
//! | `throw <message>` | uncaught plain error, exit 1 |
//! | `exit <n>` | exit with code n |
//! | `flaky <name> <N> <line…>` | run `<line…>` only on the first N runs counted in `<file>.<name>.count` |
//!
//! `--inspect pipe` fakes the step debugger's line protocol (see `bmcp_jobs::debug`): it prints
//! `waiting`, answers every command, "pauses" at each breakpoint line in turn (one fake variable
//! `line`), and runs the script once no breakpoint is left.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("run") {
        eprintln!("usage: bmcp-fake-script-host run <file> [--ws url] [--input k=v]...");
        return ExitCode::from(2);
    }
    let mut script = None;
    let mut ws = String::new();
    let mut inputs = HashMap::new();
    let mut timeout = None;
    let mut inspect = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--inspect" => inspect = args.next().as_deref() == Some("pipe"),
            "--ws" => ws = args.next().unwrap_or_default(),
            "--input" => {
                let pair = args.next().unwrap_or_default();
                let (k, v) = pair.split_once('=').unwrap_or((&pair, ""));
                inputs.insert(k.to_string(), v.to_string());
            }
            "--inputs-file" => {
                // A JSON object of strings, like the real host reads.
                let path = args.next().unwrap_or_default();
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                match serde_json::from_str::<HashMap<String, String>>(&text) {
                    Ok(map) => inputs.extend(map),
                    Err(err) => {
                        eprintln!("error: bad --inputs-file {path}: {err}");
                        return ExitCode::from(2);
                    }
                }
            }
            "--timeout-secs" => {
                timeout = args.next().and_then(|s| s.parse::<f64>().ok());
            }
            path => script = Some(PathBuf::from(path)),
        }
    }
    let Some(script) = script else {
        eprintln!("error: missing <file>");
        return ExitCode::from(2);
    };
    let Ok(text) = std::fs::read_to_string(&script) else {
        eprintln!("error: script not found: {}", script.display());
        return ExitCode::from(2);
    };
    let host = Host {
        script,
        ws,
        inputs,
        timeout,
        deadline: timeout.map(|t| Instant::now() + Duration::from_secs_f64(t)),
    };
    if inspect {
        fake_debugger(&host.url());
    }
    for (n, line) in text.lines().enumerate() {
        if let Some(code) = host.line(&host.substitute(line), n as u32 + 1) {
            return ExitCode::from(code);
        }
    }
    ExitCode::SUCCESS
}

struct Host {
    script: PathBuf,
    ws: String,
    inputs: HashMap<String, String>,
    timeout: Option<f64>,
    deadline: Option<Instant>,
}

impl Host {
    fn substitute(&self, line: &str) -> String {
        let mut line = line.to_string();
        for (k, v) in &self.inputs {
            line = line.replace(&format!("{{{{{k}}}}}"), v);
        }
        line
    }

    /// Run one line; `Some(code)` = exit now.
    fn line(&self, line: &str, n: u32) -> Option<u8> {
        let mut rest = line.trim();
        // `when key=value ...` conditions.
        while let Some(cond) = rest.strip_prefix("when ") {
            let (pair, tail) = cond.split_once(' ').unwrap_or((cond, ""));
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if self.inputs.get(k).map(String::as_str).unwrap_or("") != v {
                return None;
            }
            rest = tail.trim();
        }
        let (cmd, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        match cmd {
            "" | "#" | "//" => None,
            "out" => {
                println!("{arg}");
                let _ = std::io::stdout().flush();
                None
            }
            "err" => {
                eprintln!("{arg}");
                None
            }
            "result" => {
                println!("##bmcp:result {arg}");
                None
            }
            "time" => {
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis();
                println!("##bmcp:result {{\"{arg}\": {ms}}}");
                None
            }
            "args" => {
                let inputs: BTreeMap<&String, &String> = self.inputs.iter().collect();
                // BMCP_TAB_DEFAULTS is JSON already (or absent → null).
                let tab_defaults: serde_json::Value = std::env::var("BMCP_TAB_DEFAULTS")
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(serde_json::Value::Null);
                let result = serde_json::json!({
                    "ws": self.ws,
                    "inputs": inputs,
                    "timeout_secs": self.timeout.unwrap_or(0.0),
                    "tab_defaults": tab_defaults,
                });
                println!("##bmcp:result {result}");
                None
            }
            "sleep" => {
                let wanted = Duration::from_millis(arg.trim().parse().unwrap_or(0));
                match self.deadline {
                    Some(deadline) if Instant::now() + wanted > deadline => {
                        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                        eprintln!("error: script timed out and was terminated");
                        Some(124)
                    }
                    _ => {
                        std::thread::sleep(wanted);
                        None
                    }
                }
            }
            "fail" => {
                let mut parts = arg.splitn(3, ' ');
                let code = parts.next().unwrap_or("NOT_FOUND");
                let command = parts.next().unwrap_or("page.find");
                let message = parts.next().unwrap_or("failed");
                eprintln!(
                    "error: Uncaught Error: browser.{command} failed: {code}: {message}\n    at {}:{n}:7",
                    self.url()
                );
                Some(1)
            }
            "throw" => {
                eprintln!("error: Uncaught Error: {arg}\n    at {}:{n}:7", self.url());
                Some(1)
            }
            "exit" => Some(arg.trim().parse().unwrap_or(1)),
            "flaky" => {
                let mut parts = arg.splitn(3, ' ');
                let name = parts.next().unwrap_or("x");
                let times: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
                let inner = parts.next().unwrap_or("");
                if bump(&self.script, name) <= times {
                    self.line(inner, n)
                } else {
                    None
                }
            }
            other => {
                eprintln!("error: Uncaught Error: unknown fake command {other:?}");
                Some(1)
            }
        }
    }

    fn url(&self) -> String {
        format!("file:///{}", self.script.display()).replace('\\', "/")
    }
}

/// The `--inspect pipe` protocol, minus any real debugging. Returns when the script should run.
fn fake_debugger(url: &str) {
    use std::io::BufRead;
    let say = |msg: serde_json::Value| {
        println!("##bmcp:debug {msg}");
        let _ = std::io::stdout().flush();
    };
    say(serde_json::json!({ "method": "waiting", "params": { "url": url } }));
    let mut stops: Vec<u64> = Vec::new();
    let mut paused = false;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { return };
        let msg: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let id = msg["id"].clone();
        if msg["method"] == "set_breakpoints" {
            stops = msg["params"]["lines"]
                .as_array()
                .map(|a| a.iter().filter_map(|l| l.as_u64()).collect())
                .unwrap_or_default();
            stops.sort_unstable();
            say(serde_json::json!({ "id": id, "result": { "lines": stops } }));
            continue;
        }
        say(serde_json::json!({ "id": id, "result": {} }));
        if std::mem::take(&mut paused) {
            say(serde_json::json!({ "method": "resumed", "params": {} }));
        }
        if stops.is_empty() {
            return;
        }
        let at = stops.remove(0);
        paused = true;
        say(serde_json::json!({ "method": "paused", "params": {
            "reason": "breakpoint", "line": at, "column": 1, "url": url, "hit_breakpoints": [at],
            "call_frames": [{ "function": "(top level)", "line": at, "url": url }],
            "scopes": [{ "type": "module", "variables": [
                { "name": "line", "type": "number", "value": at.to_string() }
            ] }],
        } }));
    }
}

/// Increment the counter `<script>.<name>.count` and return the new value.
fn bump(script: &Path, name: &str) -> u32 {
    let path = PathBuf::from(format!("{}.{name}.count", script.display()));
    let count = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
        + 1;
    let _ = std::fs::write(&path, count.to_string());
    count
}
