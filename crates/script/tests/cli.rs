//! End-to-end tests: run the real `bmcp-script-host` binary (MockSink) and
//! check its output and exit code.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// Repo root (this crate lives in `<root>/crates/script`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Run `bmcp-script-host run <script> <extra...>`.
fn run(script: PathBuf, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bmcp-script-host"))
        .arg("run")
        .arg(script)
        .args(extra)
        .output()
        .expect("failed to start bmcp-script-host")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

#[test]
fn hello_lists_tabs_loops_and_reads_inputs() {
    let script = repo_root().join("scripts/examples/hello.ts");
    let out = run(
        script,
        &["--mock", "--input", "name=Ada", "--input", "times=3"],
    );
    let stdout = text(&out.stdout);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        out.status,
        text(&out.stderr)
    );

    let expected = r#"hello, Ada
found 2 tabs
- 1: Example Domain <https://example.com/>
- 2: GitHub <https://github.com/>
goto #1 -> {"called":"nav.goto","args":{"url":"https://example.com/?page=1"}}
goto #2 -> {"called":"nav.goto","args":{"url":"https://example.com/?page=2"}}
goto #3 -> {"called":"nav.goto","args":{"url":"https://example.com/?page=3"}}
page.text -> {"called":"page.text","args":null}
done
"#;
    assert_eq!(stdout, expected);
}

#[test]
fn uncaught_error_exits_1_with_ts_stack() {
    let script = repo_root().join("scripts/examples/error.ts");
    let out = run(script, &[]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{stderr}");
    assert!(text(&out.stdout).contains("got 2 tabs"));
    assert!(
        stderr.contains(r#"error: Uncaught Error: step "click login" has no selector"#),
        "stderr:\n{stderr}"
    );
    // Source maps: positions are in the .ts file (throw on line 13, call on line 20).
    assert!(stderr.contains("at checkStep ("), "stderr:\n{stderr}");
    assert!(stderr.contains("error.ts:13:11"), "stderr:\n{stderr}");
    assert!(stderr.contains("error.ts:20:1"), "stderr:\n{stderr}");
}

#[test]
fn sink_errors_are_catchable_and_proxies_are_not_thenables() {
    let script = repo_root().join("crates/script/tests/fixtures/sink_error.ts");
    let out = run(script, &[]);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "stderr:\n{}", text(&out.stderr));
    assert_eq!(
        stdout,
        "caught: browser.test.fail failed: mock failure, args={\"why\":\"testing\"} (code -32000)\n\
         {\"called\":\"x.y\",\"args\":{\"a\":1}}\n\
         object\n"
    );
}

/// Shared check for both timeout flavours.
fn assert_times_out(fixture: &str, first_line: &str) {
    let script = repo_root()
        .join("crates/script/tests/fixtures")
        .join(fixture);
    let started = Instant::now();
    let out = run(script, &["--timeout-secs", "1"]);
    let took = started.elapsed();
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(124), "stderr:\n{stderr}");
    assert!(stderr.contains("timed out"), "stderr:\n{stderr}");
    assert_eq!(text(&out.stdout), format!("{first_line}\n"));
    // 1 s limit + process start; generous bound for slow CI machines.
    assert!(took < Duration::from_secs(15), "took {took:?}");
}

#[test]
fn timeout_terminates_busy_loop() {
    assert_times_out("busy_loop.ts", "spinning");
}

#[test]
fn timeout_stops_pending_await() {
    assert_times_out("await_forever.ts", "waiting");
}

#[test]
fn missing_file_is_usage_error() {
    let out = run(repo_root().join("does/not/exist.ts"), &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("script not found"));
}

#[test]
fn output_prints_result_lines() {
    let script = repo_root().join("crates/script/tests/fixtures/output.ts");
    let out = run(script, &["--input", "email=a@x.test"]);
    assert!(out.status.success(), "stderr:\n{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "##bmcp:result {\"email\":\"a@x.test\",\"step\":1}\nplain log line\n##bmcp:result {\"step\":2}\n"
    );
}
