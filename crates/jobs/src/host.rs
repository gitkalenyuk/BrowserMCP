//! Running one script in a `bmcp-script-host` process and reading what it printed.
//!
//! ```text
//! bmcp-script-host run <file.ts> --ws ws://127.0.0.1:<port>/v1/ws --input k=v ... --timeout-secs N
//! exit codes: 0 ok, 1 uncaught script error, 2 usage/setup error, 124 timeout
//! ```
//!
//! **Structured results.** A script reports data with `browser.output({...})`, which prints
//! one stdout line `##bmcp:result {json}`. Several lines are merged: object keys are combined
//! (later values win), any other value replaces what came before. All other stdout lines are
//! ordinary logs (only a tail is kept).
//!
//! **Errors.** On an uncaught exception the host prints `error: Uncaught Error: …` and the TS
//! stack to stderr. A failed browser call reads
//! `browser.page.find failed: NOT_FOUND: no element matches …`; [`parse_failure`] pulls out the
//! command (`page.find`), the code (`NOT_FOUND`) and the script line of the top stack frame.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use bmcp_core::{CmdError, ErrorCode};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;

/// Prefix of a structured-result line on stdout.
pub const RESULT_PREFIX: &str = "##bmcp:result ";

/// File name of the real script host binary.
pub const HOST_EXE: &str = if cfg!(windows) {
    "bmcp-script-host.exe"
} else {
    "bmcp-script-host"
};

/// Extra time after `--timeout-secs` before we kill the process ourselves (the host normally
/// stops on its own and exits with 124).
const KILL_GRACE: Duration = Duration::from_secs(5);
/// How much of stdout / stderr we keep.
const STDOUT_TAIL: usize = 4 * 1024;
const STDERR_TAIL: usize = 16 * 1024;

/// What the host printed when a script failed, split into parts an agent can act on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptFailure {
    /// Browser error code (`NOT_FOUND`, `TIMEOUT`, …) when a browser call failed; otherwise
    /// `SCRIPT_ERROR`, `SCRIPT_TIMEOUT`, `HOST_ERROR` or `CANCELLED`.
    pub code: String,
    /// The browser command that failed, e.g. `page.find`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// First line of the error message.
    pub message: String,
    /// Line in the script of the top stack frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl ScriptFailure {
    fn new(code: &str, message: impl Into<String>) -> Self {
        ScriptFailure {
            code: code.to_string(),
            command: None,
            message: message.into(),
            line: None,
        }
    }

    /// Errors that usually mean "the page changed and the script's selectors are broken":
    /// a `page.*` command that could not find or use its element. Such failures pause a run
    /// and ask for a patch (self-healing) instead of just failing the row.
    pub fn needs_heal(&self) -> bool {
        const HEAL_CODES: &[&str] = &[
            "NOT_FOUND",
            "NOT_INTERACTABLE",
            "STALE_REF",
            "TIMEOUT",
            "AMBIGUOUS_TARGET",
        ];
        self.command
            .as_deref()
            .is_some_and(|c| c.starts_with("page."))
            && HEAL_CODES.contains(&self.code.as_str())
    }
}

/// How one script run ended.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HostOutcome {
    /// True when the script finished without an uncaught error (exit code 0).
    pub ok: bool,
    /// Process exit code (0 ok, 1 script error, 2 setup error, 124 timeout); `None` when the
    /// process was killed.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// Merged `browser.output(...)` values; `null` when the script reported nothing.
    pub result: Value,
    /// Last few KB of ordinary stdout lines.
    pub stdout: String,
    /// Last few KB of stderr.
    pub stderr: String,
    /// Set when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScriptFailure>,
}

/// Where the host binary is and where it should send browser calls.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Explicit host binary. `None` = search (see [`locate_host`]).
    pub exe: Option<PathBuf>,
    /// The browser's WebSocket endpoint, `ws://127.0.0.1:<port>/v1/ws`.
    pub ws_url: String,
}

/// Find `bmcp-script-host`: the explicit path, then `$BMCP_SCRIPT_HOST`, then next to the
/// running executable (the normal install layout, and `target/<profile>/` in a cargo build),
/// then one folder up (test binaries live in `target/<profile>/deps/`), then
/// `$CARGO_TARGET_DIR/{debug,release}`.
pub fn locate_host(explicit: Option<&Path>) -> Result<PathBuf, CmdError> {
    if let Some(path) = explicit {
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(host_missing(&format!("{} does not exist", path.display())))
        };
    }
    let mut candidates = Vec::new();
    if let Some(env) = std::env::var_os("BMCP_SCRIPT_HOST") {
        candidates.push(PathBuf::from(env));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(HOST_EXE));
        if let Some(up) = dir.parent() {
            candidates.push(up.join(HOST_EXE));
        }
    }
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        let target = PathBuf::from(target);
        candidates.push(target.join("debug").join(HOST_EXE));
        candidates.push(target.join("release").join(HOST_EXE));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| host_missing("not found next to the browser executable"))
}

fn host_missing(detail: &str) -> CmdError {
    CmdError::new(
        ErrorCode::NotFound,
        format!("script host {HOST_EXE}: {detail}"),
    )
    .with_hint(
        "Build it with `cargo build -p bmcp-script` and keep it next to browsermcp.exe, or set \
         BMCP_SCRIPT_HOST to its path.",
    )
}

/// One script run.
pub struct HostRun<'a> {
    pub exe: &'a Path,
    pub ws_url: &'a str,
    pub script: &'a Path,
    pub inputs: &'a [(String, String)],
    pub timeout: Duration,
}

/// Start the host, wait for it (or for `cancel`), collect output. Never returns `Err`: a
/// process that cannot even start is reported as a failed outcome with code `HOST_ERROR`.
pub async fn run_host(run: HostRun<'_>, cancel: &CancellationToken) -> HostOutcome {
    let started = Instant::now();
    let mut cmd = tokio::process::Command::new(run.exe);
    cmd.arg("run")
        .arg(run.script)
        .arg("--ws")
        .arg(run.ws_url)
        .arg("--timeout-secs")
        .arg(run.timeout.as_secs_f64().max(0.001).to_string());
    for (key, value) in run.inputs {
        cmd.arg("--input").arg(format!("{key}={value}"));
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Dropping the child (cancel, shutdown) kills the process.
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // The release browser is a GUI app: don't flash a console window per script.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let failed = |failure: ScriptFailure, stderr: String| HostOutcome {
        ok: false,
        exit_code: None,
        duration_ms: started.elapsed().as_millis() as u64,
        result: Value::Null,
        stdout: String::new(),
        stderr,
        error: Some(failure),
    };

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let message = format!("cannot start {}: {err}", run.exe.display());
            return failed(ScriptFailure::new("HOST_ERROR", &message), message);
        }
    };
    // Both pipes are read while the process runs, so it never blocks on a full pipe.
    let stdout = tokio::spawn(read_stdout(child.stdout.take()));
    let stderr = tokio::spawn(read_tail(child.stderr.take(), STDERR_TAIL));

    let status = tokio::select! {
        status = child.wait() => Some(status),
        _ = tokio::time::sleep(run.timeout + KILL_GRACE) => None,
        _ = cancel.cancelled() => None,
    };
    if status.is_none() {
        let _ = child.kill().await;
    }
    let (result, stdout) = stdout.await.unwrap_or_default();
    let stderr = stderr.await.unwrap_or_default();
    let duration_ms = started.elapsed().as_millis() as u64;

    let exit_code = match status {
        Some(Ok(status)) => status.code(),
        Some(Err(err)) => {
            let message = format!("waiting for the script host failed: {err}");
            return failed(ScriptFailure::new("HOST_ERROR", message), stderr);
        }
        None => None,
    };
    let error = match exit_code {
        Some(0) => None,
        Some(124) => Some(ScriptFailure::new(
            "SCRIPT_TIMEOUT",
            format!("script timed out after {:?}", run.timeout),
        )),
        Some(1) => Some(parse_failure(&stderr)),
        Some(code) => Some(ScriptFailure::new(
            "HOST_ERROR",
            first_error_line(&stderr)
                .unwrap_or_else(|| format!("script host exited with code {code}")),
        )),
        None if cancel.is_cancelled() => Some(ScriptFailure::new("CANCELLED", "run cancelled")),
        None => Some(ScriptFailure::new(
            "SCRIPT_TIMEOUT",
            format!("script host did not stop after {:?}; killed", run.timeout),
        )),
    };
    HostOutcome {
        ok: error.is_none(),
        exit_code,
        duration_ms,
        result,
        stdout,
        stderr,
        error,
    }
}

/// Reads stdout line by line: result lines are merged into one value, the rest is kept as a
/// tail.
async fn read_stdout(pipe: Option<impl AsyncRead + Unpin>) -> (Value, String) {
    let mut result = Value::Null;
    let mut tail = String::new();
    let Some(pipe) = pipe else {
        return (result, tail);
    };
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match line.strip_prefix(RESULT_PREFIX) {
            Some(json) => match serde_json::from_str::<Value>(json) {
                Ok(value) => merge_result(&mut result, value),
                // A broken result line is still useful as a log line.
                Err(_) => push_tail(&mut tail, &line, STDOUT_TAIL),
            },
            None => push_tail(&mut tail, &line, STDOUT_TAIL),
        }
    }
    (result, tail)
}

async fn read_tail(pipe: Option<impl AsyncRead + Unpin>, max: usize) -> String {
    let mut tail = String::new();
    if let Some(pipe) = pipe {
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            push_tail(&mut tail, &line, max);
        }
    }
    tail
}

/// Append a line, then drop whole lines from the front while over `max` bytes.
fn push_tail(tail: &mut String, line: &str, max: usize) {
    tail.push_str(line);
    tail.push('\n');
    while tail.len() > max {
        match tail.find('\n') {
            Some(end) if end + 1 < tail.len() => {
                tail.drain(..=end);
            }
            // One huge line: keep its end.
            _ => {
                let mut cut = tail.len() - max;
                while !tail.is_char_boundary(cut) {
                    cut += 1;
                }
                tail.drain(..cut);
            }
        }
    }
}

/// See the [module docs](self): objects merge key by key, anything else replaces.
pub fn merge_result(acc: &mut Value, value: Value) {
    match (acc, value) {
        (Value::Object(acc), Value::Object(new)) => acc.extend(new),
        (acc, value) => *acc = value,
    }
}

/// `browser.page.find failed: NOT_FOUND: message`
static BROWSER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"browser\.([A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)+) failed: ([A-Z][A-Z0-9_]+): ").unwrap()
});
/// `    at checkStep (file:///C:/x/login.ts:13:11)` or `    at file:///C:/x/login.ts:20:1`
static STACK_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*at .*:(\d+):(\d+)\)?\s*$").unwrap());

/// Split the host's stderr after an uncaught error (exit code 1) into a [`ScriptFailure`].
pub fn parse_failure(stderr: &str) -> ScriptFailure {
    // The host prints the uncaught error last, as "error: <message>" + stack lines. Search
    // from the end so console.error lines the script printed earlier don't confuse us.
    let lines: Vec<&str> = stderr.lines().collect();
    let Some(start) = lines.iter().rposition(|l| l.starts_with("error: ")) else {
        let message = lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map_or("script failed", |l| l.trim());
        return ScriptFailure::new("SCRIPT_ERROR", message);
    };
    let message = lines[start]["error: ".len()..].trim().to_string();
    let line = lines[start + 1..]
        .iter()
        .find_map(|l| STACK_FRAME.captures(l))
        .and_then(|c| c[1].parse().ok());
    let (code, command) = match BROWSER_ERROR.captures(&message) {
        Some(c) => (c[2].to_string(), Some(c[1].to_string())),
        None => ("SCRIPT_ERROR".to_string(), None),
    };
    ScriptFailure {
        code,
        command,
        message,
        line,
    }
}

fn first_error_line(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .find_map(|l| l.strip_prefix("error: "))
        .map(|l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_failed_browser_call() {
        let stderr = "\
some console.error line
error: Uncaught Error: browser.page.find failed: NOT_FOUND: no element matches name \"Sign in\"
    at login (file:///C:/data/scripts/login.ts:7:9)
    at file:///C:/data/scripts/login.ts:20:1
";
        let f = parse_failure(stderr);
        assert_eq!(f.code, "NOT_FOUND");
        assert_eq!(f.command.as_deref(), Some("page.find"));
        assert_eq!(f.line, Some(7));
        assert!(f.message.contains("no element matches"));
        assert!(f.needs_heal());
    }

    #[test]
    fn plain_errors_do_not_need_healing() {
        let f = parse_failure("error: Uncaught Error: wrong password\n    at file:///x.ts:3:7\n");
        assert_eq!(f.code, "SCRIPT_ERROR");
        assert_eq!(f.command, None);
        assert_eq!(f.line, Some(3));
        assert!(!f.needs_heal());
        // A browser error outside page.* (e.g. nav.goto TIMEOUT) is not a selector problem.
        let f = parse_failure("error: Uncaught Error: browser.nav.goto failed: TIMEOUT: slow\n");
        assert_eq!(f.code, "TIMEOUT");
        assert!(!f.needs_heal());
        assert_eq!(parse_failure("").code, "SCRIPT_ERROR");
    }

    #[test]
    fn results_merge() {
        let mut acc = Value::Null;
        merge_result(&mut acc, json!({"a": 1}));
        merge_result(&mut acc, json!({"b": 2, "a": 3}));
        assert_eq!(acc, json!({"a": 3, "b": 2}));
        merge_result(&mut acc, json!("text"));
        assert_eq!(acc, json!("text"));
    }

    #[test]
    fn tail_keeps_the_end() {
        let mut tail = String::new();
        for i in 0..100 {
            push_tail(&mut tail, &format!("line {i}"), 30);
        }
        assert!(tail.len() <= 30);
        assert!(tail.ends_with("line 99\n"));
        let mut tail = String::new();
        push_tail(&mut tail, &"é".repeat(40), 11);
        assert!(tail.len() <= 11 && tail.ends_with('\n'));
    }
}
