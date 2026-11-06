//! Step debugger for scripts: `bmcp-script-host run <file> --inspect <pipe|port>`.
//!
//! deno_core embeds V8's inspector (the engine behind Chrome DevTools). We open ONE inspector
//! session and drive it from a small "controller" thread. The controller speaks raw CDP
//! (`Debugger.*`, `Runtime.getProperties`) to V8 and a much smaller, line-based protocol to
//! the client (the browser core, a test, a person with `nc`), with everything in TypeScript
//! line numbers (mapped through the transpiler's source map).
//!
//! # Client protocol (one JSON object per line)
//!
//! Client -> host (`id` is optional; with an `id` the host answers `{"id", "result"|"error"}`):
//!
//! ```text
//! {"id":1,"method":"set_breakpoints","params":{"lines":[3,10]}}   -> {"id":1,"result":{"lines":[3,10]}}
//! {"id":2,"method":"continue"}        run (or start) until the next breakpoint
//! {"id":3,"method":"step_over"}       F10   (before the start: break on the first statement)
//! {"id":4,"method":"step_into"}       F11
//! {"id":5,"method":"step_out"}        Shift+F11
//! {"id":6,"method":"pause"}           break as soon as possible
//! ```
//!
//! Host -> client events:
//!
//! ```text
//! {"method":"waiting","params":{"url":"file:///…/x.ts"}}   loaded, nothing ran yet
//! {"method":"paused","params":{"reason":"breakpoint","line":10,"column":3,"url":"…",
//!     "hit_breakpoints":[10],
//!     "call_frames":[{"function":"add","line":6,"url":"…"}, …],
//!     "scopes":[{"type":"local","variables":[{"name":"x","type":"number","value":"1"}]}, …]}}
//! {"method":"resumed"}
//! {"method":"ended","params":{"status":"completed"|"failed"|"timed_out","error":"…"?}}
//! ```
//!
//! Lines and columns are 1-based (like editors). `scopes` lists the top frame's scopes
//! without the global one; variables are only one level deep and every value is a short,
//! truncated string.
//!
//! Transports: `pipe` = commands on stdin, events on stdout as `##bmcp:debug <json>` lines
//! (between the script's own output); `<port>` = a TCP socket on 127.0.0.1 (one client).
//!
//! # Threads
//!
//! ```text
//! V8 thread (host.rs)          controller thread (here)          client
//!   load module                  Debugger.enable ...  ---------->  "waiting"
//!   wait_for_runtime_run_...  <- Runtime.runIfWaitingForDebugger <- continue
//!   evaluate, hits breakpoint
//!   (parked inside V8)        -> Debugger.paused
//!                               Runtime.getProperties (served while paused)
//!                                                     ---------->  "paused"
//!                             <- Debugger.stepOver              <- step_over
//! ```
//!
//! While V8 is paused its thread sits inside the inspector's message loop, so the controller
//! MUST live on another thread; the inspector's channels are thread-safe and wake V8 up.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread::JoinHandle;

use deno_core::futures::StreamExt;
use deno_core::futures::channel::mpsc as fmpsc;
use deno_core::serde_json::{self, Value, json};
use deno_core::sourcemap::SourceMap;
use deno_core::{
    InspectorMsg, InspectorSessionChannels, InspectorSessionKind, InspectorSessionProxy, JsRuntime,
    ModuleLoader, ModuleSpecifier,
};

/// Prefix of a protocol line on stdout in `--inspect pipe` mode.
pub const DEBUG_PREFIX: &str = "##bmcp:debug ";

/// Longest value string in `scopes` (longer ones end with `…`).
const MAX_VALUE_CHARS: usize = 120;
/// At most this many variables per scope.
const MAX_VARIABLES: usize = 60;

/// The client side of a debug session: commands in, protocol lines out.
pub struct DebugChannel {
    /// One JSON command per message. Closing the sender = the client is gone.
    pub commands: mpsc::Receiver<String>,
    /// Sends one JSON line (without newline) to the client.
    pub send: Box<dyn FnMut(&str) + Send>,
}

impl DebugChannel {
    /// `--inspect pipe`: commands from stdin, events on stdout as `##bmcp:debug <json>`.
    pub fn pipe() -> Self {
        let (tx, commands) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if !line.trim().is_empty() && tx.send(line).is_err() {
                    break;
                }
            }
        });
        DebugChannel {
            commands,
            send: Box::new(|line| {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{DEBUG_PREFIX}{line}");
                let _ = out.flush();
            }),
        }
    }

    /// `--inspect <port>`: listen on 127.0.0.1 and wait for ONE client (port 0 = any free
    /// port). Prints `debugger listening on 127.0.0.1:<port>` to stderr first.
    pub fn tcp(port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        eprintln!("debugger listening on {}", listener.local_addr()?);
        let (stream, _) = listener.accept()?;
        Self::from_stream(stream)
    }

    fn from_stream(stream: TcpStream) -> std::io::Result<Self> {
        let reader = stream.try_clone()?;
        let mut writer = stream;
        let (tx, commands) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if !line.trim().is_empty() && tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(DebugChannel {
            commands,
            send: Box::new(move |line| {
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }),
        })
    }
}

/// How the script ended, for the final `ended` event.
pub struct EndInfo {
    /// `completed`, `failed` or `timed_out`.
    pub status: &'static str,
    pub error: Option<String>,
}

/// A running controller; [`Attached::finish`] sends `ended` and waits for the thread.
pub struct Attached {
    inputs: mpsc::Sender<Input>,
    thread: Option<JoinHandle<()>>,
}

impl Attached {
    pub fn finish(mut self, end: EndInfo) {
        let _ = self.inputs.send(Input::Finished(end));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Connect the debugger to a runtime whose main module is LOADED but not yet evaluated, and
/// block until the client says go (`continue` / a step / `pause`).
///
/// `runtime` must have been created with `RuntimeOptions { inspector: true, .. }`.
pub fn attach(
    runtime: &mut JsRuntime,
    loader: &dyn ModuleLoader,
    main: &ModuleSpecifier,
    channel: DebugChannel,
) -> Attached {
    // The TS -> JS line mapping of the main module (the loader made it while loading).
    let lines = LineMap::new(loader.get_source_map(main.as_str()).as_deref());

    // One inspector session, connected through two channels:
    //   to_v8   (String)        controller -> V8
    //   from_v8 (InspectorMsg)  V8 -> controller
    let (to_v8, to_v8_rx) = fmpsc::unbounded::<String>();
    let (from_v8_tx, mut from_v8) = fmpsc::unbounded::<InspectorMsg>();
    let inspector = runtime.inspector();
    let _ = inspector
        .get_session_sender()
        .unbounded_send(InspectorSessionProxy {
            channels: InspectorSessionChannels::Regular {
                tx: from_v8_tx,
                rx: to_v8_rx,
            },
            // NonBlocking: the session does not keep the event loop alive.
            kind: InspectorSessionKind::NonBlocking {
                wait_for_disconnect: false,
            },
        });

    // Everything the controller reacts to goes through ONE queue, so its logic is a plain
    // sequential loop. Two small forwarder threads feed it.
    let (inputs, inputs_rx) = mpsc::channel::<Input>();
    {
        let inputs = inputs.clone();
        std::thread::spawn(move || {
            // `block_on` is fine: this thread does nothing else.
            while let Some(msg) = deno_core::futures::executor::block_on(from_v8.next()) {
                if inputs.send(Input::V8(msg.content)).is_err() {
                    return;
                }
            }
            let _ = inputs.send(Input::V8Gone);
        });
    }
    {
        let inputs = inputs.clone();
        let commands = channel.commands;
        std::thread::spawn(move || {
            while let Ok(line) = commands.recv() {
                if inputs.send(Input::Client(line)).is_err() {
                    return;
                }
            }
            let _ = inputs.send(Input::ClientGone);
        });
    }

    let mut controller = Controller {
        inputs: inputs_rx,
        backlog: VecDeque::new(),
        to_v8,
        next_id: 1,
        send: channel.send,
        url: main.to_string(),
        lines,
        breakpoints: HashMap::new(),
        scripts: HashMap::new(),
        trace: std::env::var_os("BMCP_DEBUG_TRACE").is_some(),
        started: false,
        detached: false,
    };
    let thread = std::thread::spawn(move || controller.run());

    // Block (processing inspector messages) until the controller sends
    // `Runtime.runIfWaitingForDebugger`. Breakpoints are set by then.
    inspector.wait_for_runtime_run_if_waiting_for_debugger();
    // deno_core asserts that nobody else holds the inspector when the runtime is dropped.
    drop(inspector);

    Attached {
        inputs,
        thread: Some(thread),
    }
}

/// Everything the controller thread waits for.
enum Input {
    /// A CDP message from V8 (response or notification), JSON text.
    V8(String),
    /// The inspector session closed (runtime dropped).
    V8Gone,
    /// A command line from the client.
    Client(String),
    /// The client disconnected.
    ClientGone,
    /// The script is done (sent by [`Attached::finish`]).
    Finished(EndInfo),
}

struct Controller {
    inputs: mpsc::Receiver<Input>,
    /// Inputs that arrived while we waited for a CDP response; handled first.
    backlog: VecDeque<Input>,
    to_v8: fmpsc::UnboundedSender<String>,
    next_id: u64,
    send: Box<dyn FnMut(&str) + Send>,
    /// URL of the main module (what breakpoints are set on).
    url: String,
    lines: LineMap,
    /// V8 breakpoint id -> TS line (1-based), for `hit_breakpoints`.
    breakpoints: HashMap<String, u32>,
    /// V8 script id -> URL (from `Debugger.scriptParsed`; call frames only carry the id).
    scripts: HashMap<String, String>,
    /// `BMCP_DEBUG_TRACE=1`: print the raw CDP traffic to stderr.
    trace: bool,
    /// `Runtime.runIfWaitingForDebugger` was sent.
    started: bool,
    /// The client is gone: run to the end without stopping.
    detached: bool,
}

impl Controller {
    fn run(&mut self) {
        // Debugger on; never stop inside deno_core's or our prelude's JS (`ext:` URLs).
        let _ = self.cdp("Debugger.enable", json!({}));
        let _ = self.cdp(
            "Debugger.setBlackboxPatterns",
            json!({ "patterns": ["^ext:", "^node:"] }),
        );
        self.emit("waiting", json!({ "url": self.url }));

        while let Some(input) = self.next_input() {
            match input {
                Input::V8(text) => self.on_v8(&text),
                Input::Client(line) => self.on_client(&line),
                Input::ClientGone => self.detach(),
                Input::V8Gone => break,
                Input::Finished(end) => {
                    self.emit("ended", json!({ "status": end.status, "error": end.error }));
                    break;
                }
            }
        }
    }

    fn next_input(&mut self) -> Option<Input> {
        self.backlog.pop_front().or_else(|| self.inputs.recv().ok())
    }

    /// Send one CDP request and wait for its response. Other inputs that arrive meanwhile
    /// are kept (in order) for the main loop.
    fn cdp(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        if self.trace {
            eprintln!("[cdp] -> {request}");
        }
        self.to_v8
            .unbounded_send(request.to_string())
            .map_err(|_| "the script has ended".to_string())?;
        let mut kept = VecDeque::new();
        let result = loop {
            let input = match self.inputs.recv() {
                Ok(input) => input,
                Err(_) => break Err("the script has ended".to_string()),
            };
            match input {
                Input::V8(text) => {
                    let msg: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    if msg["id"].as_u64() == Some(id) {
                        break match msg.get("error") {
                            Some(err) => Err(err["message"]
                                .as_str()
                                .unwrap_or("inspector error")
                                .to_string()),
                            None => Ok(msg["result"].clone()),
                        };
                    }
                    kept.push_back(Input::V8(text));
                }
                // The script finished while we waited: no answer will come.
                end @ (Input::Finished(_) | Input::V8Gone) => {
                    kept.push_back(end);
                    break Err("the script has ended".to_string());
                }
                other => kept.push_back(other),
            }
        };
        self.backlog.extend(kept);
        result
    }

    fn emit(&mut self, method: &str, params: Value) {
        if !self.detached {
            (self.send)(&json!({ "method": method, "params": params }).to_string());
        }
    }

    fn reply(&mut self, id: &Value, result: Result<Value, String>) {
        if id.is_null() || self.detached {
            return;
        }
        let msg = match result {
            Ok(result) => json!({ "id": id, "result": result }),
            Err(message) => json!({ "id": id, "error": { "message": message } }),
        };
        (self.send)(&msg.to_string());
    }

    // ---------------------------------------------------------------- client commands

    fn on_client(&mut self, line: &str) {
        let msg: Value = match serde_json::from_str(line) {
            Ok(msg) => msg,
            Err(err) => {
                self.emit("error", json!({ "message": format!("bad command: {err}") }));
                return;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg["method"].as_str().unwrap_or_default().to_string();
        let result = match method.as_str() {
            "set_breakpoints" => self.set_breakpoints(&msg["params"]["lines"]),
            "continue" => self.resume("Debugger.resume", false),
            "step_over" => self.resume("Debugger.stepOver", true),
            "step_into" => self.resume("Debugger.stepInto", true),
            "step_out" => self.resume("Debugger.stepOut", true),
            "pause" => self.pause(),
            other => Err(format!(
                "unknown method {other:?} (use continue, step_over, step_into, step_out, \
                 pause or set_breakpoints)"
            )),
        };
        self.reply(&id, result);
    }

    /// `continue` / `step_*`. Before the start a step means "break on the first statement".
    fn resume(&mut self, cdp_method: &str, break_first: bool) -> Result<Value, String> {
        if !self.started {
            if break_first {
                self.cdp("Debugger.pause", json!({}))?;
            }
            return self.start();
        }
        self.cdp(cdp_method, json!({}))?;
        Ok(json!({}))
    }

    fn pause(&mut self) -> Result<Value, String> {
        self.cdp("Debugger.pause", json!({}))?;
        if !self.started {
            return self.start();
        }
        Ok(json!({}))
    }

    /// Let the V8 thread evaluate the module.
    fn start(&mut self) -> Result<Value, String> {
        self.started = true;
        self.cdp("Runtime.runIfWaitingForDebugger", json!({}))?;
        Ok(json!({}))
    }

    /// Replace all breakpoints. Returns the TS lines they really landed on (an empty line
    /// or a type-only line moves to the next line with code; lines past the end are dropped).
    fn set_breakpoints(&mut self, lines: &Value) -> Result<Value, String> {
        let wanted: Vec<u32> = lines
            .as_array()
            .ok_or("params.lines must be an array of line numbers")?
            .iter()
            .filter_map(|l| l.as_u64())
            .filter(|&l| l >= 1)
            .map(|l| l as u32)
            .collect();
        for id in std::mem::take(&mut self.breakpoints).into_keys() {
            let _ = self.cdp("Debugger.removeBreakpoint", json!({ "breakpointId": id }));
        }
        let mut resolved = Vec::new();
        for ts_line in wanted {
            let Some(js) = self.lines.ts_to_js(ts_line - 1) else {
                continue;
            };
            let answer = self.cdp(
                "Debugger.setBreakpointByUrl",
                json!({ "url": self.url, "lineNumber": js.line, "columnNumber": js.column }),
            );
            let Ok(answer) = answer else { continue };
            // V8 may move the breakpoint to the next breakable spot; report where it is.
            let actual = answer["locations"][0]
                .as_object()
                .and_then(|loc| {
                    let line = loc.get("lineNumber")?.as_u64()? as u32;
                    let column = loc.get("columnNumber")?.as_u64()? as u32;
                    self.lines.js_to_ts(line, column)
                })
                .unwrap_or(js.ts_line)
                + 1;
            if let Some(id) = answer["breakpointId"].as_str() {
                self.breakpoints.insert(id.to_string(), actual);
            }
            if !resolved.contains(&actual) {
                resolved.push(actual);
            }
        }
        resolved.sort_unstable();
        Ok(json!({ "lines": resolved }))
    }

    /// The client is gone: drop breakpoints and let the script finish on its own.
    fn detach(&mut self) {
        self.detached = true;
        for id in std::mem::take(&mut self.breakpoints).into_keys() {
            let _ = self.cdp("Debugger.removeBreakpoint", json!({ "breakpointId": id }));
        }
        if self.started {
            // Also resumes if paused.
            let _ = self.cdp("Debugger.disable", json!({}));
        } else {
            let _ = self.start();
        }
    }

    // ---------------------------------------------------------------- V8 notifications

    fn on_v8(&mut self, text: &str) {
        if self.trace {
            eprintln!("[cdp] <- {text}");
        }
        let msg: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        match msg["method"].as_str() {
            Some("Debugger.scriptParsed") => {
                let params = &msg["params"];
                if let (Some(id), Some(url)) = (params["scriptId"].as_str(), params["url"].as_str())
                {
                    self.scripts.insert(id.to_string(), url.to_string());
                }
            }
            Some("Debugger.paused") => self.on_paused(&msg["params"]),
            Some("Debugger.resumed") => self.emit("resumed", json!({})),
            // scriptParsed, contexts, late responses: not interesting here.
            _ => {}
        }
    }

    fn on_paused(&mut self, params: &Value) {
        let frames = params["callFrames"].as_array().cloned().unwrap_or_default();
        if self.detached {
            let _ = self.cdp("Debugger.resume", json!({}));
            return;
        }
        // Paused in deno_core / prelude code (e.g. `pause` while an op ran): step on until
        // we are back in a user file.
        let in_user_code = frames
            .first()
            .is_some_and(|f| self.frame_url(f).starts_with("file:"));
        if !in_user_code {
            let _ = self.cdp("Debugger.stepInto", json!({}));
            return;
        }

        let call_frames: Vec<Value> = frames.iter().map(|f| self.frame_info(f)).collect();
        let top = &frames[0];
        let location = &top["location"];
        let top_url = self.frame_url(top);
        let (line, column) = self.ts_position(
            &top_url,
            location["lineNumber"].as_u64().unwrap_or(0) as u32,
            location["columnNumber"].as_u64().unwrap_or(0) as u32,
        );
        let hit: Vec<u32> = params["hitBreakpoints"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.breakpoints.get(id.as_str()?).copied())
                    .collect()
            })
            .unwrap_or_default();
        let reason = match params["reason"].as_str() {
            _ if !hit.is_empty() => "breakpoint",
            Some("other") | None => "step",
            Some(other) => other,
        }
        .to_string();
        let scopes = self.scopes(top);
        self.emit(
            "paused",
            json!({
                "reason": reason,
                "line": line,
                "column": column,
                "url": top_url,
                "hit_breakpoints": hit,
                "call_frames": call_frames,
                "scopes": scopes,
            }),
        );
    }

    /// `{function, line, url}` of one CDP call frame (line 1-based, TS when mappable).
    fn frame_info(&self, frame: &Value) -> Value {
        let url = self.frame_url(frame);
        let (line, _) = self.ts_position(
            &url,
            frame["location"]["lineNumber"].as_u64().unwrap_or(0) as u32,
            frame["location"]["columnNumber"].as_u64().unwrap_or(0) as u32,
        );
        let name = frame["functionName"].as_str().unwrap_or_default();
        json!({
            "function": if name.is_empty() { "(top level)" } else { name },
            "line": line,
            "url": url,
        })
    }

    /// URL of a call frame's script (newer V8 leaves the frame's own `url` empty).
    fn frame_url(&self, frame: &Value) -> String {
        match frame["url"].as_str() {
            Some(url) if !url.is_empty() => url.to_string(),
            _ => frame["location"]["scriptId"]
                .as_str()
                .and_then(|id| self.scripts.get(id))
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// 0-based JS position -> 1-based TS line/column (main module), else 1-based JS.
    fn ts_position(&self, url: &str, line: u32, column: u32) -> (u32, u32) {
        if url == self.url
            && let Some(ts) = self.lines.js_to_ts_pos(line, column)
        {
            return (ts.0 + 1, ts.1 + 1);
        }
        (line + 1, column + 1)
    }

    /// The top frame's scopes (not `global`), one level of variables each.
    fn scopes(&mut self, frame: &Value) -> Vec<Value> {
        let mut scopes = Vec::new();
        for scope in frame["scopeChain"].as_array().cloned().unwrap_or_default() {
            let kind = scope["type"].as_str().unwrap_or("scope").to_string();
            let Some(object_id) = scope["object"]["objectId"].as_str() else {
                continue;
            };
            if kind == "global" {
                continue;
            }
            let props = self.cdp(
                "Runtime.getProperties",
                json!({ "objectId": object_id, "ownProperties": true, "generatePreview": true }),
            );
            let variables: Vec<Value> = props
                .ok()
                .and_then(|p| p["result"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .take(MAX_VARIABLES)
                .map(|p| {
                    let (kind, value) = describe(&p["value"]);
                    json!({ "name": p["name"], "type": kind, "value": value })
                })
                .collect();
            scopes.push(json!({ "type": kind, "variables": variables }));
        }
        scopes
    }
}

/// `(type, short text)` of a CDP `RemoteObject`, e.g. `("number", "42")`,
/// `("string", "\"hi\"")`, `("object", "{id: 1, url: \"…\"}")`.
fn describe(obj: &Value) -> (String, String) {
    let kind = obj["type"].as_str().unwrap_or("undefined");
    let subtype = obj["subtype"].as_str();
    let text = match (kind, subtype) {
        // A `let` read before its declaration ran.
        (_, _) if obj.is_null() => "<uninitialized>".to_string(),
        ("undefined", _) => "undefined".to_string(),
        (_, Some("null")) => "null".to_string(),
        ("string", _) => {
            serde_json::to_string(obj["value"].as_str().unwrap_or_default()).unwrap_or_default()
        }
        ("number" | "boolean", _) => match obj.get("value") {
            Some(v) => v.to_string(),
            // NaN, Infinity, -0 come as `unserializableValue`.
            None => obj["unserializableValue"]
                .as_str()
                .unwrap_or("?")
                .to_string(),
        },
        ("function", _) => {
            let first = obj["description"].as_str().unwrap_or("function");
            first.lines().next().unwrap_or("function").to_string()
        }
        ("object", _) => match obj.get("preview") {
            Some(preview) => preview_text(preview),
            None => obj["description"].as_str().unwrap_or("Object").to_string(),
        },
        _ => obj["description"].as_str().unwrap_or(kind).to_string(),
    };
    let kind = subtype
        .filter(|s| *s == "array" || *s == "null")
        .unwrap_or(kind);
    (kind.to_string(), truncate(&text, MAX_VALUE_CHARS))
}

/// `{a: 1, b: "x"}` / `[1, 2]` / `Map(2) {…}` from an `ObjectPreview`.
fn preview_text(preview: &Value) -> String {
    let props = preview["properties"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let is_array = preview["subtype"].as_str() == Some("array");
    let mut parts: Vec<String> = props
        .iter()
        .map(|p| {
            let value = p["value"].as_str().unwrap_or_default();
            let value = if p["type"].as_str() == Some("string") {
                format!("\"{value}\"")
            } else {
                value.to_string()
            };
            if is_array {
                value
            } else {
                format!("{}: {value}", p["name"].as_str().unwrap_or("?"))
            }
        })
        .collect();
    if preview["overflow"].as_bool() == Some(true) {
        parts.push("…".to_string());
    }
    let body = parts.join(", ");
    let description = preview["description"].as_str().unwrap_or("Object");
    if is_array {
        format!("{description} [{body}]")
    } else if description == "Object" {
        format!("{{{body}}}")
    } else {
        format!("{description} {{{body}}}")
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

// -------------------------------------------------------------------- line mapping

/// Where a TS line starts in the generated JS (all 0-based).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsPos {
    pub line: u32,
    pub column: u32,
    /// The TS line that was actually mapped (the asked one, or the next line with code).
    pub ts_line: u32,
}

/// TS <-> JS positions of one module, from the transpiler's source map. Without a map
/// (plain `.js`) positions are the same in both.
pub struct LineMap {
    map: Option<SourceMap>,
}

impl LineMap {
    pub fn new(source_map: Option<&[u8]>) -> Self {
        LineMap {
            map: source_map.and_then(|bytes| SourceMap::from_slice(bytes).ok()),
        }
    }

    /// First generated position of TS line `ts_line` (0-based); a line without code (blank,
    /// comment, `interface …`) moves to the next line that has some. `None` past the end.
    pub fn ts_to_js(&self, ts_line: u32) -> Option<JsPos> {
        let Some(map) = &self.map else {
            return Some(JsPos {
                line: ts_line,
                column: 0,
                ts_line,
            });
        };
        // The earliest mapping of the nearest TS line >= ts_line.
        let mut best: Option<(u32, u32, u32)> = None; // (src_line, dst_line, dst_col)
        for token in map.tokens() {
            if !token.has_source() {
                continue;
            }
            let src = token.get_src_line();
            if src < ts_line {
                continue;
            }
            let candidate = (src, token.get_dst_line(), token.get_dst_col());
            if best.is_none_or(|b| candidate < b) {
                best = Some(candidate);
            }
        }
        best.map(|(src, line, column)| JsPos {
            line,
            column,
            ts_line: src,
        })
    }

    /// TS line (0-based) of a generated position.
    pub fn js_to_ts(&self, line: u32, column: u32) -> Option<u32> {
        self.js_to_ts_pos(line, column).map(|(l, _)| l)
    }

    /// TS `(line, column)` (0-based) of a generated position.
    pub fn js_to_ts_pos(&self, line: u32, column: u32) -> Option<(u32, u32)> {
        let Some(map) = &self.map else {
            return Some((line, column));
        };
        let token = map.lookup_token(line, column)?;
        token
            .has_source()
            .then(|| (token.get_src_line(), token.get_src_col()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_values() {
        let d = |v: Value| describe(&v);
        assert_eq!(
            d(json!({"type":"number","value":3})),
            ("number".into(), "3".into())
        );
        assert_eq!(
            d(json!({"type":"string","value":"hi"})),
            ("string".into(), "\"hi\"".into())
        );
        assert_eq!(
            d(json!({"type":"object","subtype":"null","value":null})),
            ("null".into(), "null".into())
        );
        assert_eq!(
            d(json!({"type":"undefined"})),
            ("undefined".into(), "undefined".into())
        );
        assert_eq!(
            d(json!({"type":"number","unserializableValue":"NaN","description":"NaN"})).1,
            "NaN"
        );
        let obj = json!({"type":"object","className":"Object","description":"Object","preview":{
            "type":"object","description":"Object","overflow":false,"properties":[
                {"name":"id","type":"number","value":"1"},
                {"name":"url","type":"string","value":"https://x/"}]}});
        assert_eq!(d(obj).1, "{id: 1, url: \"https://x/\"}");
        let arr = json!({"type":"object","subtype":"array","description":"Array(2)","preview":{
            "type":"object","subtype":"array","description":"Array(2)","overflow":false,
            "properties":[{"name":"0","type":"number","value":"1"},{"name":"1","type":"number","value":"2"}]}});
        assert_eq!(d(arr), ("array".into(), "Array(2) [1, 2]".into()));
        let long = "x".repeat(500);
        assert_eq!(
            d(json!({"type":"string","value":long})).1.chars().count(),
            MAX_VALUE_CHARS
        );
    }

    #[test]
    fn identity_map_without_source_map() {
        let map = LineMap::new(None);
        assert_eq!(
            map.ts_to_js(4),
            Some(JsPos {
                line: 4,
                column: 0,
                ts_line: 4
            })
        );
        assert_eq!(map.js_to_ts(7, 2), Some(7));
    }
}
