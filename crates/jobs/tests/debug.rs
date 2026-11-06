//! `script.debug_start` / `script.debug_cmd`: the proxy between the registry and a
//! `--inspect pipe` host (here the fake host, which speaks the same line protocol).

mod common;

use bmcp_core::ErrorCode;
use common::{Env, wait_event};
use serde_json::json;

#[tokio::test]
async fn pauses_continues_and_reports_output_and_end() {
    let env = Env::new();
    env.save("dbg", "out hello\nresult {\"a\": 1}\nout bye")
        .await;
    let mut rx = env.events.subscribe();

    let started = env
        .ok(
            "script.debug_start",
            json!({"name": "dbg", "breakpoints": [3, 1]}),
        )
        .await;
    let id = started["id"].as_str().unwrap().to_string();
    assert_eq!(started["breakpoints"], json!([1, 3]));
    assert!(started.get("error").is_none());

    let paused = wait_event(&mut rx, "script.debug_paused", |d| d["id"] == id).await;
    assert_eq!(paused.data["line"], 1);
    assert_eq!(paused.data["scopes"][0]["variables"][0]["name"], "line");

    // Breakpoints can change while paused; the answer says where they are.
    let set = env
        .ok(
            "script.debug_cmd",
            json!({"id": id, "action": "set_breakpoints", "lines": [2]}),
        )
        .await;
    assert_eq!(set["breakpoints"], json!([2]));

    env.ok("script.debug_cmd", json!({"id": id, "action": "continue"}))
        .await;
    wait_event(&mut rx, "script.debug_resumed", |d| d["id"] == id).await;
    let paused = wait_event(&mut rx, "script.debug_paused", |d| d["id"] == id).await;
    assert_eq!(paused.data["line"], 2);

    env.ok("script.debug_cmd", json!({"id": id, "action": "step_over"}))
        .await;
    let out = wait_event(&mut rx, "script.debug_output", |d| d["id"] == id).await;
    assert_eq!(out.data["stream"], "stdout");
    assert_eq!(out.data["text"], "hello");
    let ended = wait_event(&mut rx, "script.debug_ended", |d| d["id"] == id).await;
    assert_eq!(ended.data["ok"], true);
    assert_eq!(ended.data["exit_code"], 0);
    assert_eq!(ended.data["result"], json!({"a": 1}));
    assert_eq!(ended.data["stopped"], false);

    // The session is gone.
    let err = env
        .call("script.debug_cmd", json!({"id": id, "action": "continue"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn inline_code_errors_end_with_a_parsed_failure() {
    let env = Env::new();
    let mut rx = env.events.subscribe();
    let started = env
        .ok(
            "script.debug_start",
            json!({"code": "args\nthrow boom", "inputs": {"k": "v"}}),
        )
        .await;
    let id = started["id"].as_str().unwrap().to_string();
    let ended = wait_event(&mut rx, "script.debug_ended", |d| d["id"] == id).await;
    assert_eq!(ended.data["ok"], false);
    // Inputs reach the host (through the --inputs-file).
    assert_eq!(ended.data["result"]["inputs"], json!({"k": "v"}));
    assert_eq!(ended.data["error"]["code"], "SCRIPT_ERROR");
    assert_eq!(ended.data["error"]["line"], 2);
    assert!(
        ended.data["error"]["message"]
            .as_str()
            .unwrap()
            .contains("boom")
    );
    // The temporary file next to the saved scripts is cleaned up.
    let left: Vec<_> = std::fs::read_dir(env.root().join("scripts"))
        .unwrap()
        .flatten()
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn stop_kills_a_paused_script() {
    let env = Env::new();
    env.save("dbg", "out never").await;
    let mut rx = env.events.subscribe();
    let started = env
        .ok(
            "script.debug_start",
            json!({"name": "dbg", "breakpoints": [1]}),
        )
        .await;
    let id = started["id"].as_str().unwrap().to_string();
    wait_event(&mut rx, "script.debug_paused", |d| d["id"] == id).await;
    env.ok("script.debug_cmd", json!({"id": id, "action": "stop"}))
        .await;
    let ended = wait_event(&mut rx, "script.debug_ended", |d| d["id"] == id).await;
    assert_eq!(ended.data["stopped"], true);
    assert_eq!(ended.data["ok"], false);
}

#[tokio::test]
async fn bad_arguments() {
    let env = Env::new();
    let err = env.call("script.debug_start", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = env
        .call("script.debug_start", json!({"name": "missing"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = env
        .call(
            "script.debug_cmd",
            json!({"id": "dbg999", "action": "jump"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

/// The same flow against the REAL V8 host (no browser: the script makes no browser calls).
/// Runs only when `BMCP_REAL_SCRIPT_HOST` points at a built `bmcp-script-host`
/// (`cargo build -p bmcp-script`); bmcp-jobs does not depend on the V8 crate.
#[tokio::test]
async fn real_host_pauses_with_ts_lines_and_variables() {
    let Some(exe) = std::env::var_os("BMCP_REAL_SCRIPT_HOST") else {
        eprintln!("skipped: set BMCP_REAL_SCRIPT_HOST to a bmcp-script-host binary");
        return;
    };
    // The host connects to the browser's WebSocket at start: give it an idle one.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().route(
        "/v1/ws",
        axum::routing::get(|ws: axum::extract::WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let tmp = tempfile::tempdir().unwrap();
    let mut config = common::config(tmp.path());
    config.host_exe = Some(exe.into());
    config.ws_url = format!("ws://127.0.0.1:{port}/v1/ws");
    let env = Env::with_config(tmp, config);
    // Line 2 is type-only (gone in the JS), so its breakpoint lands on line 3.
    let code = "// demo\ntype Row = { n: number };\nconst rows: Row[] = [{ n: 2 }, { n: 5 }];\n\
                let sum = 0;\nfor (const row of rows) {\n  sum += row.n;\n}\n\
                console.log(`sum=${sum} name=${inputs.name}`);\nbrowser.output({ sum });\n";
    let mut rx = env.events.subscribe();
    let started = env
        .ok(
            "script.debug_start",
            json!({"code": code, "breakpoints": [2, 6], "inputs": {"name": "Ada"}}),
        )
        .await;
    let id = started["id"].as_str().unwrap().to_string();
    assert_eq!(started["breakpoints"], json!([3, 6]));

    let paused = wait_event(&mut rx, "script.debug_paused", |d| d["id"] == id).await;
    assert_eq!(paused.data["line"], 3, "{}", paused.data);
    env.ok("script.debug_cmd", json!({"id": id, "action": "continue"}))
        .await;
    let paused = wait_event(&mut rx, "script.debug_paused", |d| d["id"] == id).await;
    assert_eq!(paused.data["line"], 6);
    let vars = paused.data["scopes"].to_string();
    assert!(vars.contains(r#""value":"{n: 2}""#), "{vars}");
    assert!(vars.contains(r#""name":"sum""#), "{vars}");

    // Drop the breakpoints and run to the end.
    env.ok(
        "script.debug_cmd",
        json!({"id": id, "action": "set_breakpoints", "lines": []}),
    )
    .await;
    env.ok("script.debug_cmd", json!({"id": id, "action": "continue"}))
        .await;
    let out = wait_event(&mut rx, "script.debug_output", |d| {
        d["id"] == id && d["stream"] == "stdout"
    })
    .await;
    assert_eq!(out.data["text"], "sum=7 name=Ada");
    let ended = wait_event(&mut rx, "script.debug_ended", |d| d["id"] == id).await;
    assert_eq!(ended.data["ok"], true, "{}", ended.data);
    assert_eq!(ended.data["result"], json!({"sum": 7}));
}
