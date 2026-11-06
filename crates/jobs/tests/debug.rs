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
