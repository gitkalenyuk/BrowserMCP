//! `bmcp-fake-script-host`: a stand-in for `bmcp-script-host` in the tests (no V8, starts in
//! milliseconds). Same command line:
//!
//! ```text
//! bmcp-fake-script-host run <file> [--ws <url>] [--input k=v]... [--timeout-secs N]
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
//! | `args` | `result {"ws": …, "inputs": {…}, "timeout_secs": …}` |
//! | `sleep <ms>` | sleep; past `--timeout-secs` it exits 124 like the real host |
//! | `fail <CODE> <command> <message>` | uncaught failed browser call, exit 1 |
//! | `throw <message>` | uncaught plain error, exit 1 |
//! | `exit <n>` | exit with code n |
//! | `flaky <name> <N> <line…>` | run `<line…>` only on the first N runs counted in `<file>.<name>.count` |

use std::collections::HashMap;
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
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ws" => ws = args.next().unwrap_or_default(),
            "--input" => {
                let pair = args.next().unwrap_or_default();
                let (k, v) = pair.split_once('=').unwrap_or((&pair, ""));
                inputs.insert(k.to_string(), v.to_string());
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
                let mut inputs: Vec<_> = self.inputs.iter().collect();
                inputs.sort();
                let inputs: Vec<String> = inputs
                    .iter()
                    .map(|(k, v)| format!("{k:?}: {v:?}"))
                    .collect();
                println!(
                    "##bmcp:result {{\"ws\": {:?}, \"inputs\": {{{}}}, \"timeout_secs\": {}}}",
                    self.ws,
                    inputs.join(", "),
                    self.timeout.unwrap_or(0.0)
                );
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
