//! Step-debugging a script from the UI (or any API client): `script.debug_start` and
//! `script.debug_cmd`.
//!
//! The core does no debugging itself. It starts `bmcp-script-host run <file> --inspect pipe`
//! and proxies the host's small line protocol (crates/script/src/debugger.rs):
//!
//! ```text
//! script.debug_start ──spawn──> bmcp-script-host --inspect pipe
//!                    stdin  <── {"id":1,"method":"set_breakpoints","params":{"lines":[…]}}
//!                    stdin  <── {"id":2,"method":"continue"}
//! script.debug_cmd   stdin  <── {"id":n,"method":"step_over"}
//!                    stdout ──> ##bmcp:debug {"method":"paused",…}   -> event script.debug_paused
//!                    stdout ──> ##bmcp:debug {"method":"resumed"}    -> event script.debug_resumed
//!                    stdout ──> ordinary lines / ##bmcp:result        -> event script.debug_output
//!                    process exit                                     -> event script.debug_ended
//! ```
//!
//! All lines are TypeScript lines (1-based). A session ends when the script ends, on
//! `script.debug_cmd {action: "stop"}`, after `timeout_secs` (default 30 min) or on browser
//! shutdown.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bmcp_core::{CmdError, ErrorCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::Inner;
use crate::host::{RESULT_PREFIX, ScriptFailure, locate_host, merge_result, parse_failure};

/// Prefix of a debugger protocol line on the host's stdout (same as
/// `bmcp_script::DEBUG_PREFIX`; that crate is not a dependency, it pulls in V8).
pub const DEBUG_PREFIX: &str = "##bmcp:debug ";

/// Event topics of debug sessions (`events.subscribe {"topics":["script.*"]}`).
pub mod topics {
    /// `{id, reason, line, column, url, hit_breakpoints, call_frames, scopes}`
    pub const PAUSED: &str = "script.debug_paused";
    /// `{id}`
    pub const RESUMED: &str = "script.debug_resumed";
    /// `{id, stream: "stdout"|"stderr"|"result", text}`
    pub const OUTPUT: &str = "script.debug_output";
    /// `{id, ok, exit_code, duration_ms, result, stopped, error?}`
    pub const ENDED: &str = "script.debug_ended";
}

/// Default lifetime of a debug session.
pub const DEFAULT_DEBUG_TIMEOUT_SECS: u64 = 30 * 60;
/// How long to wait for the host to load the script / to answer one command.
const START_WAIT: Duration = Duration::from_secs(30);
const COMMAND_WAIT: Duration = Duration::from_secs(15);
const STDERR_TAIL: usize = 16 * 1024;

/// The live debug sessions of this browser.
#[derive(Default)]
pub struct DebugSessions {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl DebugSessions {
    fn get(&self, id: &str) -> Result<Arc<Session>, CmdError> {
        self.lock().get(id).cloned().ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotFound,
                format!("debug session '{id}' not found (it may have ended)"),
            )
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One `bmcp-script-host --inspect pipe` process.
struct Session {
    stdin: tokio::sync::Mutex<ChildStdin>,
    /// Requests sent to the host, waiting for `{"id": n, ...}`.
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    next_id: AtomicU64,
    /// Kills the process (`stop`, timeout, shutdown).
    kill: CancellationToken,
}

impl Session {
    /// Send one command to the host and wait for its answer.
    async fn request(&self, method: &str, params: Value) -> Result<Value, CmdError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock_pending().insert(id, tx);
        let line = json!({ "id": id, "method": method, "params": params }).to_string() + "\n";
        let written = {
            let mut stdin = self.stdin.lock().await;
            match stdin.write_all(line.as_bytes()).await {
                Ok(()) => stdin.flush().await,
                Err(err) => Err(err),
            }
        };
        if written.is_err() {
            self.lock_pending().remove(&id);
            return Err(ended_error());
        }
        match tokio::time::timeout(COMMAND_WAIT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(message))) => Err(CmdError::new(ErrorCode::InvalidParams, message)),
            // The reader dropped the sender: the process ended.
            Ok(Err(_)) => Err(ended_error()),
            Err(_) => {
                self.lock_pending().remove(&id);
                Err(CmdError::new(
                    ErrorCode::Timeout,
                    format!("the script host did not answer `{method}`"),
                ))
            }
        }
    }

    fn lock_pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Result<Value, String>>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn ended_error() -> CmdError {
    CmdError::new(ErrorCode::NotFound, "the debug session has ended")
}

// ------------------------------------------------------------------ commands

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugStartArgs {
    /// Saved script to debug. Give `name` or `code`.
    pub name: Option<String>,
    /// Inline TypeScript instead of a saved script (e.g. the editor's unsaved buffer).
    pub code: Option<String>,
    /// Values the script reads as inputs.<key> (passed as strings).
    pub inputs: Option<serde_json::Map<String, Value>>,
    /// TypeScript lines (1-based) to stop at.
    pub breakpoints: Option<Vec<u32>>,
    /// End the session after this many seconds (default 1800).
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DebugStartOutput {
    /// Session id for script.debug_cmd and the script.debug_* events.
    pub id: String,
    /// Where the breakpoints really are (a line without code moves to the next one).
    pub breakpoints: Vec<u32>,
    /// Set when the script could not even start (e.g. a syntax error); the session is gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScriptFailure>,
}

/// What `script.debug_cmd` can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugAction {
    /// Run to the next breakpoint (F5).
    Continue,
    /// Next line, over calls (F10).
    StepOver,
    /// Next line, into calls (F11).
    StepInto,
    /// Run until the current function returns (Shift+F11).
    StepOut,
    /// Stop as soon as possible.
    Pause,
    /// Replace all breakpoints with `lines`.
    SetBreakpoints,
    /// Kill the script.
    Stop,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DebugCmdArgs {
    /// Session id from script.debug_start.
    pub id: String,
    pub action: DebugAction,
    /// For set_breakpoints: TypeScript lines (1-based).
    pub lines: Option<Vec<u32>>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DebugCmdOutput {
    /// For set_breakpoints: where the breakpoints really are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakpoints: Option<Vec<u32>>,
}

impl Inner {
    /// `script.debug_start`.
    pub(crate) async fn debug_start(
        self: &Arc<Self>,
        args: DebugStartArgs,
        inputs: Vec<(String, String)>,
    ) -> Result<DebugStartOutput, CmdError> {
        let exe = locate_host(self.config.host_exe.as_deref())?;
        let (path, temporary) = match (&args.name, &args.code) {
            (Some(name), None) => (self.scripts.require(name)?, false),
            (None, Some(code)) => (self.write_temp_script(code)?, true),
            _ => {
                return Err(CmdError::invalid_params(
                    "give exactly one of `name` or `code`",
                ));
            }
        };

        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("run")
            .arg(&path)
            .arg("--ws")
            .arg(self.ws_url())
            .arg("--inspect")
            .arg("pipe");
        for (key, value) in &inputs {
            cmd.arg("--input").arg(format!("{key}={value}"));
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|err| CmdError::internal(format!("cannot start {}: {err}", exe.display())))?;

        let id = format!("dbg{}", NEXT_SESSION.fetch_add(1, Ordering::Relaxed));
        let session = Arc::new(Session {
            stdin: tokio::sync::Mutex::new(child.stdin.take().expect("piped stdin")),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            kill: self.shutdown.child_token(),
        });
        self.debug.lock().insert(id.clone(), session.clone());

        // Readers: stdout (protocol + output) and stderr (output + the final error).
        let (waiting_tx, waiting_rx) = oneshot::channel::<()>();
        let stdout = tokio::spawn(read_stdout(
            self.clone(),
            id.clone(),
            session.clone(),
            child.stdout.take().expect("piped stdout"),
            waiting_tx,
        ));
        let stderr = tokio::spawn(read_stderr(
            self.clone(),
            id.clone(),
            child.stderr.take().expect("piped stderr"),
        ));

        // Driver: wait for the end (or kill), then report and clean up.
        let (ended_tx, ended_rx) = oneshot::channel::<Value>();
        let limit = Duration::from_secs(
            args.timeout_secs
                .unwrap_or(DEFAULT_DEBUG_TIMEOUT_SECS)
                .max(1),
        );
        {
            let inner = self.clone();
            let id = id.clone();
            let session = session.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                let status = tokio::select! {
                    status = child.wait() => status.ok(),
                    _ = session.kill.cancelled() => None,
                    _ = tokio::time::sleep(limit) => None,
                };
                if status.is_none() {
                    let _ = child.kill().await;
                }
                let result = stdout.await.unwrap_or(Value::Null);
                let stderr = stderr.await.unwrap_or_default();
                inner.debug.lock().remove(&id);
                if temporary {
                    let _ = std::fs::remove_file(&path);
                }
                let exit_code = status.and_then(|s| s.code());
                let error = match exit_code {
                    Some(0) => None,
                    Some(1) => Some(parse_failure(&stderr)),
                    _ if status.is_none() => None,
                    _ => Some(parse_failure(&stderr)),
                };
                let ended = json!({
                    "id": id,
                    "ok": exit_code == Some(0),
                    "exit_code": exit_code,
                    "duration_ms": started.elapsed().as_millis() as u64,
                    "result": result,
                    "stopped": status.is_none(),
                    "error": error,
                });
                inner.events.emit(topics::ENDED, ended.clone());
                let _ = ended_tx.send(ended);
            });
        }

        // Wait until the script is loaded (or the process died, e.g. on a syntax error).
        tokio::select! {
            ok = waiting_rx => {
                if ok.is_err() {
                    // stdout closed before "waiting": the host failed to load the script.
                    let ended = ended_rx.await.unwrap_or(Value::Null);
                    let error = serde_json::from_value::<ScriptFailure>(ended["error"].clone())
                        .unwrap_or_else(|_| ScriptFailure {
                            code: "HOST_ERROR".into(),
                            command: None,
                            message: "the script host stopped before the script was loaded".into(),
                            line: None,
                        });
                    return Ok(DebugStartOutput { id, breakpoints: Vec::new(), error: Some(error) });
                }
            }
            _ = tokio::time::sleep(START_WAIT) => {
                session.kill.cancel();
                return Err(CmdError::new(ErrorCode::Timeout, "the script host did not load the script in time"));
            }
        }

        let lines = args.breakpoints.unwrap_or_default();
        let set = session
            .request("set_breakpoints", json!({ "lines": lines }))
            .await?;
        session.request("continue", json!({})).await?;
        Ok(DebugStartOutput {
            id,
            breakpoints: lines_of(&set),
            error: None,
        })
    }

    /// `script.debug_cmd`.
    pub(crate) async fn debug_cmd(&self, args: DebugCmdArgs) -> Result<DebugCmdOutput, CmdError> {
        let session = self.debug.get(&args.id)?;
        let method = match args.action {
            DebugAction::Stop => {
                session.kill.cancel();
                return Ok(DebugCmdOutput { breakpoints: None });
            }
            DebugAction::SetBreakpoints => {
                let lines = args
                    .lines
                    .ok_or_else(|| CmdError::invalid_params("set_breakpoints needs `lines`"))?;
                let set = session
                    .request("set_breakpoints", json!({ "lines": lines }))
                    .await?;
                return Ok(DebugCmdOutput {
                    breakpoints: Some(lines_of(&set)),
                });
            }
            DebugAction::Continue => "continue",
            DebugAction::StepOver => "step_over",
            DebugAction::StepInto => "step_into",
            DebugAction::StepOut => "step_out",
            DebugAction::Pause => "pause",
        };
        session.request(method, json!({})).await?;
        Ok(DebugCmdOutput { breakpoints: None })
    }
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn lines_of(result: &Value) -> Vec<u32> {
    result["lines"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l.as_u64())
                .map(|l| l as u32)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the host's stdout until it closes; returns the merged `browser.output(...)` value.
async fn read_stdout(
    inner: Arc<Inner>,
    id: String,
    session: Arc<Session>,
    pipe: tokio::process::ChildStdout,
    waiting: oneshot::Sender<()>,
) -> Value {
    let mut waiting = Some(waiting);
    let mut result = Value::Null;
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(json) = line.strip_prefix(DEBUG_PREFIX) {
            let Ok(msg) = serde_json::from_str::<Value>(json) else {
                continue;
            };
            // A response to `Session::request`.
            if let Some(n) = msg["id"].as_u64() {
                if let Some(tx) = session.lock_pending().remove(&n) {
                    let answer = match msg.get("error") {
                        Some(err) => Err(err["message"].as_str().unwrap_or("error").to_string()),
                        None => Ok(msg["result"].clone()),
                    };
                    let _ = tx.send(answer);
                }
                continue;
            }
            let params = &msg["params"];
            match msg["method"].as_str() {
                Some("waiting") => {
                    if let Some(tx) = waiting.take() {
                        let _ = tx.send(());
                    }
                }
                Some("paused") => {
                    let mut event = params.clone();
                    if let Some(obj) = event.as_object_mut() {
                        obj.insert("id".into(), json!(id));
                    }
                    inner.events.emit(topics::PAUSED, event);
                }
                Some("resumed") => {
                    inner.events.emit(topics::RESUMED, json!({ "id": id }));
                }
                Some("error") => {
                    let message = params["message"].as_str().unwrap_or_default();
                    output(&inner, &id, "stderr", message);
                }
                // "ended": the process exit reports it (with exit code and parsed error).
                _ => {}
            }
        } else if let Some(json) = line.strip_prefix(RESULT_PREFIX) {
            match serde_json::from_str::<Value>(json) {
                Ok(value) => {
                    merge_result(&mut result, value);
                    output(&inner, &id, "result", json);
                }
                Err(_) => output(&inner, &id, "stdout", &line),
            }
        } else {
            output(&inner, &id, "stdout", &line);
        }
    }
    // Pending requests get "ended" (their senders are dropped).
    session.lock_pending().clear();
    result
}

/// Forwards stderr lines as output events and keeps a tail for [`parse_failure`].
async fn read_stderr(inner: Arc<Inner>, id: String, pipe: tokio::process::ChildStderr) -> String {
    let mut tail = String::new();
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        output(&inner, &id, "stderr", &line);
        tail.push_str(&line);
        tail.push('\n');
        if tail.len() > STDERR_TAIL {
            let cut = tail.len() - STDERR_TAIL;
            let cut = (cut..tail.len())
                .find(|&i| tail.is_char_boundary(i))
                .unwrap_or(0);
            tail.drain(..cut);
        }
    }
    tail
}

fn output(inner: &Inner, id: &str, stream: &str, text: &str) {
    inner.events.emit(
        topics::OUTPUT,
        json!({ "id": id, "stream": stream, "text": text }),
    );
}
