//! The step debugger (`--inspect`) without a browser: scripts run against `MockSink`, the
//! test is the debug client (see src/debugger.rs for the protocol).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use bmcp_script::{DebugChannel, MockSink, RunOptions, RunOutcome, run_script_debug};
use deno_core::serde_json::{self, Value, json};

const WAIT: Duration = Duration::from_secs(30);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// One debugged script on its own thread (one V8 isolate per thread), plus the client side.
struct Session {
    commands: Option<mpsc::Sender<String>>,
    lines: mpsc::Receiver<String>,
    /// Events read while waiting for a response.
    events: VecDeque<Value>,
    next_id: u64,
    thread: Option<JoinHandle<RunOutcome>>,
}

impl Session {
    fn start(script: PathBuf) -> Session {
        let (commands, commands_rx) = mpsc::channel::<String>();
        let (lines_tx, lines) = mpsc::channel::<String>();
        let thread = std::thread::spawn(move || {
            let channel = DebugChannel {
                commands: commands_rx,
                send: Box::new(move |line| {
                    let _ = lines_tx.send(line.to_string());
                }),
            };
            let tokio = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let opts = RunOptions {
                script,
                inputs: Vec::new(),
                timeout: None,
            };
            let (outcome, _) = tokio
                .block_on(run_script_debug(opts, Rc::new(MockSink), channel))
                .expect("run_script_debug");
            outcome
        });
        Session {
            commands: Some(commands),
            lines,
            events: VecDeque::new(),
            next_id: 1,
            thread: Some(thread),
        }
    }

    fn read(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(WAIT)
            .expect("no message from the debugger");
        serde_json::from_str(&line).expect("debugger sent invalid JSON")
    }

    /// Send a command and wait for its response (`result`, or panic on `error`).
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "id": id, "method": method, "params": params });
        self.commands
            .as_ref()
            .unwrap()
            .send(msg.to_string())
            .unwrap();
        loop {
            let msg = self.read();
            if msg["id"].as_u64() == Some(id) {
                assert!(msg.get("error").is_none(), "{method} failed: {msg}");
                return msg["result"].clone();
            }
            self.events.push_back(msg);
        }
    }

    /// Next event named `method` (others, like `resumed`, are skipped).
    fn event(&mut self, method: &str) -> Value {
        loop {
            let msg = match self.events.pop_front() {
                Some(msg) => msg,
                None => self.read(),
            };
            if msg["method"] == method {
                return msg["params"].clone();
            }
        }
    }

    fn outcome(mut self) -> RunOutcome {
        self.commands.take();
        self.thread.take().unwrap().join().expect("script thread")
    }
}

/// Value of variable `name` in any scope of a `paused` event.
fn var(paused: &Value, name: &str) -> String {
    for scope in paused["scopes"].as_array().unwrap() {
        for v in scope["variables"].as_array().unwrap() {
            if v["name"] == name {
                return v["value"].as_str().unwrap().to_string();
            }
        }
    }
    panic!("no variable {name} in {paused}");
}

#[test]
fn pauses_on_breakpoints_steps_and_reads_variables() {
    let mut s = Session::start(fixture("debug.ts"));
    let waiting = s.event("waiting");
    assert!(waiting["url"].as_str().unwrap().ends_with("debug.ts"));

    // Line 2 is a type-only line (gone in the JS): its breakpoint moves to line 3.
    let set = s.call("set_breakpoints", json!({ "lines": [2, 10] }));
    assert_eq!(set["lines"], json!([3, 10]));

    s.call("continue", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 3, "{p}");
    assert_eq!(p["reason"], "breakpoint");
    assert_eq!(p["hit_breakpoints"], json!([3]));

    s.call("continue", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 10, "{p}");
    assert_eq!(var(&p, "total"), "0");
    assert_eq!(var(&p, "item"), "{n: 1}");
    assert_eq!(p["call_frames"][0]["function"], "(top level)");

    // F11 into add(): the first line of its body, with the argument.
    s.call("step_into", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 6, "{p}");
    assert_eq!(var(&p, "x"), "1");
    assert_eq!(p["call_frames"][0]["function"], "add");
    assert_eq!(p["call_frames"][1]["line"], 10);

    // F10 over `const doubled = x * 2`.
    s.call("step_over", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 7, "{p}");
    assert_eq!(var(&p, "doubled"), "2");

    // Shift+F11 back to the caller: V8 stops at the next statement there (the loop head),
    // with the result already added.
    s.call("step_out", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 9, "{p}");
    assert_eq!(p["call_frames"][0]["function"], "(top level)");
    assert_eq!(var(&p, "total"), "2");
    assert_eq!(var(&p, "items"), "Array(3) [Object, Object, Object]");

    // New breakpoints while paused, then run to the last line (after the await).
    let set = s.call("set_breakpoints", json!({ "lines": [13] }));
    assert_eq!(set["lines"], json!([13]));
    s.call("continue", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 13, "{p}");
    assert_eq!(var(&p, "total"), "12");
    assert!(var(&p, "tabs").contains("tabs"), "{p}");

    s.call("continue", json!({}));
    let ended = s.event("ended");
    assert_eq!(ended["status"], "completed");
    assert!(matches!(s.outcome(), RunOutcome::Completed));
}

#[test]
fn a_step_before_the_start_breaks_on_the_first_statement() {
    let mut s = Session::start(fixture("debug.ts"));
    s.event("waiting");
    s.call("step_over", json!({}));
    let p = s.event("paused");
    assert_eq!(p["line"], 3, "{p}");
    // `let total` is not initialized yet.
    assert_eq!(var(&p, "total"), "<uninitialized>");
    s.call("continue", json!({}));
    assert_eq!(s.event("ended")["status"], "completed");
    assert!(matches!(s.outcome(), RunOutcome::Completed));
}

#[test]
fn a_gone_client_lets_the_script_finish() {
    let mut s = Session::start(fixture("debug.ts"));
    s.event("waiting");
    s.call("set_breakpoints", json!({ "lines": [10] }));
    // No continue: the client just disappears.
    assert!(matches!(s.outcome(), RunOutcome::Completed));
}

#[test]
fn a_failing_script_ends_as_failed() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut s = Session::start(repo.join("scripts/examples/error.ts"));
    s.event("waiting");
    s.call("continue", json!({}));
    let ended = s.event("ended");
    assert_eq!(ended["status"], "failed");
    assert!(
        ended["error"].as_str().unwrap().contains("has no selector"),
        "{ended}"
    );
    assert!(matches!(s.outcome(), RunOutcome::Failed(_)));
}

#[test]
fn unknown_commands_get_an_error() {
    let mut s = Session::start(fixture("sink_error.ts"));
    s.event("waiting");
    let err = s.call("step_over", json!({}));
    assert_eq!(err, json!({}));
    s.event("paused");
    // Unknown commands answer with an error but keep the session alive.
    let id = 99;
    s.commands
        .as_ref()
        .unwrap()
        .send(json!({ "id": id, "method": "nope" }).to_string())
        .unwrap();
    loop {
        let msg = s.read();
        if msg["id"] == id {
            assert!(
                msg["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("unknown")
            );
            break;
        }
    }
    s.call("continue", json!({}));
    assert_eq!(s.event("ended")["status"], "completed");
    s.outcome();
}

/// The real binary with `--inspect pipe`: commands on stdin, `##bmcp:debug` lines on stdout
/// between the script's own output.
#[test]
fn cli_inspect_pipe() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_bmcp-script-host"))
        .arg("run")
        .arg(fixture("debug.ts"))
        .args(["--mock", "--inspect", "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start bmcp-script-host");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut plain = Vec::new();
    // Next protocol message; plain output lines are collected on the way.
    let mut next = |plain: &mut Vec<String>| -> Value {
        loop {
            let line = stdout.next().expect("stdout closed").unwrap();
            match line.strip_prefix(bmcp_script::DEBUG_PREFIX) {
                Some(json) => return serde_json::from_str(json).unwrap(),
                None => plain.push(line),
            }
        }
    };

    assert_eq!(next(&mut plain)["method"], "waiting");
    writeln!(
        stdin,
        r#"{{"id":1,"method":"set_breakpoints","params":{{"lines":[13]}}}}"#
    )
    .unwrap();
    assert_eq!(next(&mut plain)["result"]["lines"], json!([13]));
    writeln!(stdin, r#"{{"id":2,"method":"continue"}}"#).unwrap();
    let paused = loop {
        let msg = next(&mut plain);
        if msg["method"] == "paused" {
            break msg["params"].clone();
        }
    };
    assert_eq!(paused["line"], 13);
    writeln!(stdin, r#"{{"id":3,"method":"continue"}}"#).unwrap();
    let ended = loop {
        let msg = next(&mut plain);
        if msg["method"] == "ended" {
            break msg["params"].clone();
        }
    };
    assert_eq!(ended["status"], "completed");
    assert_eq!(plain, ["total 12 2"]);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
