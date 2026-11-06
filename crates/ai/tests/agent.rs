//! The API agent loop end to end: registry with built-in commands on a FakeBackend, a mocked
//! model server, events on the bus. No real model calls.

use std::sync::Arc;
use std::time::Duration;

use bmcp_ai::AiService;
use bmcp_core::events::Event;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{Caller, Ctx, EventBus, Registry};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

struct Harness {
    registry: Arc<Registry>,
    fake: Arc<FakeBackend>,
    bus: EventBus,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        registry
            .extensions()
            .insert(AiService::new(dir.path(), "http://127.0.0.1:9/mcp"));
        bmcp_ai::register(&mut registry);
        let registry = Arc::new(registry);
        bmcp_ai::attach(&registry);
        Harness {
            registry,
            fake: FakeBackend::with_tabs(&["https://a.test/"]),
            bus: EventBus::new(),
            _dir: dir,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, bmcp_core::CmdError> {
        let ctx = Ctx::new(Caller::Ui, self.fake.clone(), self.bus.clone());
        self.registry.call(ctx, name, args).await
    }
}

/// Events until (and including) the next `ai.done` of `session`.
async fn until_done(rx: &mut broadcast::Receiver<Event>, session: &str) -> Vec<Event> {
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let event = rx.recv().await.unwrap();
            let done = event.topic == "ai.done" && event.data["session"] == session;
            events.push(event);
            if done {
                return;
            }
        }
    })
    .await
    .expect("no ai.done within 20 s");
    events
}

fn topics(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .map(|e| e.topic.as_str())
        .filter(|t| t.starts_with("ai."))
        .collect()
}

/// Did the conversation sent to the model already contain a tool result?
fn has_tool_result(req: &Request) -> bool {
    let body = String::from_utf8_lossy(&req.body);
    body.contains("\"role\":\"tool\"")
        || body.contains("tool_result")
        || body.contains("functionResponse")
}

fn sse(events: &[Value]) -> ResponseTemplate {
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

/// OpenAI-compatible server: first turn calls tabs_open + system_ping, second answers.
fn openai_responder(req: &Request) -> ResponseTemplate {
    if has_tool_result(req) {
        return sse(&[
            json!({"choices": [{"index": 0, "delta": {"content": "Opened "}}]}),
            json!({"choices": [{"index": 0, "delta": {"content": "tab 2."}, "finish_reason": "stop"}]}),
        ]);
    }
    sse(&[json!({"choices": [{"index": 0, "delta": {"tool_calls": [
        {"index": 0, "id": "c1", "type": "function", "function": {"name": "tabs_open", "arguments": "{\"url\":\"https://b.test/\"}"}},
        {"index": 1, "id": "c2", "type": "function", "function": {"name": "system_ping", "arguments": "{}"}},
    ]}, "finish_reason": "tool_calls"}]})])
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_compatible_agent_runs_registry_tools() {
    let h = Harness::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(openai_responder)
        .mount(&server)
        .await;

    h.call(
        "ai.provider_set",
        json!({ "id": "custom", "base_url": format!("{}/v1", server.uri()), "model": "test-model" }),
    )
    .await
    .unwrap();
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "custom", "prompt": "open b.test" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    assert_eq!(id, "custom-1");
    assert!(info["color"].as_str().unwrap().starts_with('#'));

    let events = until_done(&mut rx, &id).await;
    assert_eq!(
        topics(&events),
        [
            "ai.message",     // user prompt echo
            "ai.session",     // running
            "ai.tool_call",   // tabs_open
            "ai.tool_result", // {"tab": 2}
            "ai.tool_call",   // system_ping
            "ai.tool_result",
            "ai.message", // "Opened " delta
            "ai.message", // "tab 2." delta
            "ai.message", // whole text
            "ai.done",
        ]
    );
    let call = events.iter().find(|e| e.topic == "ai.tool_call").unwrap();
    assert_eq!(
        call.data,
        json!({ "session": id, "name": "tabs_open", "args": {"url": "https://b.test/"} })
    );
    let result = events.iter().find(|e| e.topic == "ai.tool_result").unwrap();
    assert_eq!(result.data["ok"], true);
    assert!(
        result.data["summary"]
            .as_str()
            .unwrap()
            .contains("\"tab\":2")
    );
    let text = events.iter().rfind(|e| e.topic == "ai.message").unwrap();
    assert_eq!(text.data["text"], "Opened tab 2.");
    assert_eq!(events.last().unwrap().data["reason"], "end_turn");

    // The tools really ran, as this agent.
    assert_eq!(h.fake.tabs()[1].url, "https://b.test/");
    let started = events
        .iter()
        .find(|e| e.topic == "command.started" && e.data["name"] == "tabs.open")
        .unwrap();
    assert_eq!(
        started.data["caller"],
        json!({ "kind": "agent", "id": "custom-1" })
    );

    // Tool definitions came from the registry (core toolset).
    let first = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&first.body).unwrap();
    let names: Vec<&str> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"tabs_open") && names.contains(&"nav_goto"),
        "{names:?}"
    );
    // `ai_handoff` is the one agent-facing `ai_*` command (core toolset): "user, please …".
    assert!(names.contains(&"ai_handoff"), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n.starts_with("ai_") && *n != "ai_handoff"),
        "ai tools must not be offered by default"
    );

    // Ownership: the session recorded tab 2; it is idle again.
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    let s = &list["sessions"][0];
    assert_eq!(
        (s["id"].as_str(), s["status"].as_str()),
        (Some("custom-1"), Some("idle"))
    );
    assert_eq!(s["tabs"], json!([2]));

    // A second message continues the same conversation.
    h.call("ai.session_send", json!({ "id": id, "text": "again" }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert!(topics(&events).contains(&"ai.tool_call") || topics(&events).contains(&"ai.message"));
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    let kinds: Vec<&str> = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "user").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "done").count(), 2);
    // The transcript is also on disk.
    let file = h._dir.path().join("ai/sessions/custom-1.jsonl");
    assert!(std::fs::read_to_string(file).unwrap().lines().count() >= kinds.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_agent_tool_use_loop() {
    let h = Harness::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &Request| {
            if has_tool_result(req) {
                return sse(&[
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "pong"}}),
                    json!({"type": "content_block_stop", "index": 0}),
                    json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
                    json!({"type": "message_stop"}),
                ]);
            }
            sse(&[
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t1", "name": "system_ping", "input": {}}}),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": ""}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
                json!({"type": "message_stop"}),
            ])
        })
        .mount(&server)
        .await;
    h.call(
        "ai.provider_set",
        json!({ "id": "anthropic", "api_key": "sk-ant-test", "base_url": server.uri() }),
    )
    .await
    .unwrap();
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "anthropic", "prompt": "ping" }),
        )
        .await
        .unwrap();
    assert_eq!(info["model"], "claude-opus-5-5"); // preset default
    let events = until_done(&mut rx, "anthropic-1").await;
    let result = events.iter().find(|e| e.topic == "ai.tool_result").unwrap();
    assert_eq!(result.data["name"], "system_ping");
    assert!(
        result.data["summary"]
            .as_str()
            .unwrap()
            .contains("BrowserMCP")
    );
    assert_eq!(events.last().unwrap().data["reason"], "end_turn");
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_agent_function_call_and_tool_errors() {
    let h = Harness::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &Request| {
            let parts = if has_tool_result(req) {
                json!([{ "text": "That tab does not exist." }])
            } else {
                // A bad call: the error goes back to the model instead of failing the session.
                json!([{ "functionCall": { "name": "nav_goto", "args": { "url": "https://c.test/", "tab": 99 } } }])
            };
            ResponseTemplate::new(200).set_body_json(json!({ "candidates": [{ "content": { "role": "model", "parts": parts }, "finishReason": "STOP" }] }))
        })
        .mount(&server)
        .await;
    h.call(
        "ai.provider_set",
        json!({ "id": "gemini", "api_key": "g", "base_url": server.uri(), "model": "gemini-test" }),
    )
    .await
    .unwrap();
    let mut rx = h.bus.subscribe();
    h.call(
        "ai.session_start",
        json!({ "provider": "gemini", "prompt": "go" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, "gemini-1").await;
    let result = events.iter().find(|e| e.topic == "ai.tool_result").unwrap();
    assert_eq!(result.data["ok"], false);
    assert!(
        result.data["summary"]
            .as_str()
            .unwrap()
            .contains("TAB_NOT_FOUND")
    );
    // Tab from the (failed) call's args is still recorded as touched.
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(list["sessions"][0]["tabs"], json!([99]));
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_cancels_a_running_model_call() {
    let h = Harness::new();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;
    h.call(
        "ai.provider_set",
        json!({ "id": "ollama", "base_url": format!("{}/v1", server.uri()), "model": "llama" }),
    )
    .await
    .unwrap();
    let mut rx = h.bus.subscribe();
    h.call(
        "ai.session_start",
        json!({ "provider": "ollama", "prompt": "slow" }),
    )
    .await
    .unwrap();
    // Wait until the session is running, then stop it.
    tokio::time::timeout(Duration::from_secs(30), async {
        while rx.recv().await.unwrap().data["status"] != "running" {}
    })
    .await
    .unwrap();
    let info = h
        .call("ai.session_stop", json!({ "id": "ollama-1" }))
        .await
        .unwrap();
    assert_eq!(info["status"], "stopped");
    let events = until_done(&mut rx, "ollama-1").await;
    assert_eq!(events.last().unwrap().data["reason"], "stopped");
    let err = h
        .call(
            "ai.session_send",
            json!({ "id": "ollama-1", "text": "more" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
}

#[tokio::test(flavor = "multi_thread")]
async fn providers_listing_and_validation() {
    let h = Harness::new();
    let out = h.call("ai.providers", json!({})).await.unwrap();
    let providers = out["providers"].as_array().unwrap();
    let ids: Vec<&str> = providers
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    for id in [
        "claude-code",
        "codex",
        "anthropic",
        "openai",
        "gemini",
        "openrouter",
        "mistral",
        "deepseek",
        "xai",
        "groq",
        "ollama",
        "lmstudio",
        "custom",
    ] {
        assert!(ids.contains(&id), "{id} missing");
    }
    let anthropic = providers.iter().find(|p| p["id"] == "anthropic").unwrap();
    assert_eq!(
        (
            anthropic["configured"].as_bool(),
            anthropic["has_key"].as_bool()
        ),
        (Some(false), Some(false))
    );

    let set = h
        .call(
            "ai.provider_set",
            json!({ "id": "anthropic", "api_key": "sk-x" }),
        )
        .await
        .unwrap();
    assert_eq!(set["configured"], true);
    assert!(set.get("api_key").is_none(), "keys are never returned");
    // "" clears.
    let set = h
        .call(
            "ai.provider_set",
            json!({ "id": "anthropic", "api_key": "" }),
        )
        .await
        .unwrap();
    assert_eq!(set["has_key"], false);

    let err = h
        .call(
            "ai.session_start",
            json!({ "provider": "anthropic", "prompt": "x" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    let err = h
        .call(
            "ai.session_start",
            json!({ "provider": "nope", "prompt": "x" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
    let err = h
        .call(
            "ai.session_start",
            json!({ "provider": "ollama", "prompt": "x", "toolsets": ["bogus"] }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_test_lists_models_without_model_calls() {
    let h = Harness::new();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "id": "qwen3:8b" }] })),
        )
        .mount(&server)
        .await;
    h.call(
        "ai.provider_set",
        json!({ "id": "lmstudio", "base_url": format!("{}/v1", server.uri()) }),
    )
    .await
    .unwrap();
    let out = h
        .call("ai.provider_test", json!({ "id": "lmstudio" }))
        .await
        .unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(out["models"], json!(["qwen3:8b"]));
    let providers = h.call("ai.providers", json!({})).await.unwrap();
    let lm = providers["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "lmstudio")
        .unwrap()
        .clone();
    assert_eq!(lm["model"], "qwen3:8b");

    // Unreachable endpoint: ok=false with an error, not a command failure.
    h.call(
        "ai.provider_set",
        json!({ "id": "ollama", "base_url": "http://127.0.0.1:9/v1" }),
    )
    .await
    .unwrap();
    let out = h
        .call("ai.provider_test", json!({ "id": "ollama" }))
        .await
        .unwrap();
    assert_eq!(out["ok"], false);
    assert!(out["error"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn term_commands_through_the_registry() {
    let h = Harness::new();
    let mut rx = h.bus.subscribe();
    let (shell, args) = if cfg!(windows) {
        ("cmd", json!(["/c", "echo hello"]))
    } else {
        ("/bin/sh", json!(["-c", "echo hello"]))
    };
    let info = h
        .call(
            "term.open",
            json!({ "shell": shell, "args": args, "cols": 80, "rows": 24 }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let e = rx.recv().await.unwrap();
            if e.data["id"] != id.as_str() {
                continue;
            }
            match e.topic.as_str() {
                "term.output" => output.push_str(e.data["data"].as_str().unwrap()),
                "term.exit" => break,
                _ => {}
            }
        }
    })
    .await
    .expect("no term.exit");
    assert!(output.contains("hello"), "{output:?}");
    let list = h.call("term.list", json!({})).await.unwrap();
    assert_eq!(list["terminals"][0]["exit_code"], 0);
    h.call("term.close", json!({ "id": id })).await.unwrap();
    let err = h
        .call("term.write", json!({ "id": id, "data": "x" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
}
