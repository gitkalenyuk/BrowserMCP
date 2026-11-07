//! The user's own providers through the registry: ai.provider_add / _set / _remove / _test,
//! listing, persistence and sessions. Model servers are mocked; no real model calls.

use std::path::Path;
use std::sync::Arc;

use bmcp_ai::AiService;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{Caller, CmdError, Ctx, ErrorCode, EventBus, Registry};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Harness {
    registry: Arc<Registry>,
    fake: Arc<FakeBackend>,
    bus: EventBus,
}

impl Harness {
    /// A browser whose data folder is `dir` (a second harness on the same dir = a restart).
    fn new(dir: &Path) -> Self {
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        registry
            .extensions()
            .insert(AiService::new(dir, "http://127.0.0.1:9/mcp"));
        bmcp_ai::register(&mut registry);
        let registry = Arc::new(registry);
        bmcp_ai::attach(&registry);
        Harness {
            registry,
            fake: FakeBackend::with_tabs(&["https://a.test/"]),
            bus: EventBus::new(),
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(Caller::Ui, self.fake.clone(), self.bus.clone());
        self.registry.call(ctx, name, args).await
    }

    async fn providers(&self) -> Vec<Value> {
        let out = self.call("ai.providers", json!({})).await.unwrap();
        out["providers"].as_array().unwrap().clone()
    }

    async fn provider(&self, id: &str) -> Option<Value> {
        self.providers().await.into_iter().find(|p| p["id"] == id)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn add_list_edit_and_remove_user_providers() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path());

    // Two accounts of the same service.
    let work = h
        .call(
            "ai.provider_add",
            json!({ "name": "OpenAI (work)", "template": "openai", "api_key": "sk-work" }),
        )
        .await
        .unwrap();
    assert_eq!(work["id"], "openai-work");
    assert_eq!(work["builtin"], false);
    assert_eq!(work["template"], "openai");
    assert_eq!(work["base_url"], "https://api.openai.com/v1");
    assert_eq!(work["configured"], true);
    assert_eq!(work["has_key"], true);
    assert!(work.get("api_key").is_none(), "keys are never returned");
    let home = h
        .call(
            "ai.provider_add",
            json!({ "name": "OpenAI (home)", "template": "openai" }),
        )
        .await
        .unwrap();
    assert_eq!(home["configured"], false, "an OpenAI account needs its key");

    // Listed after the built-ins, in the order they were added.
    let ids: Vec<String> = h
        .providers()
        .await
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(&ids[ids.len() - 2..], ["openai-work", "openai-home"]);
    let openai = h.provider("openai").await.unwrap();
    assert_eq!(
        (openai["builtin"].as_bool(), openai["template"].is_null()),
        (Some(true), true)
    );

    // Clear errors.
    let dup = h
        .call(
            "ai.provider_add",
            json!({ "name": "openai (WORK)", "template": "openai" }),
        )
        .await
        .unwrap_err();
    assert_eq!(dup.code, ErrorCode::InvalidParams);
    assert!(dup.message.contains("already exists"), "{}", dup.message);
    let bad_url = h
        .call(
            "ai.provider_add",
            json!({ "name": "Box", "base_url": "localhost:8000" }),
        )
        .await
        .unwrap_err();
    assert!(
        bad_url.message.contains("bad base URL"),
        "{}",
        bad_url.message
    );
    let missing_key = h
        .call(
            "ai.session_start",
            json!({ "provider": "openai-home", "prompt": "hi" }),
        )
        .await
        .unwrap_err();
    assert!(
        missing_key.message.contains("needs an API key"),
        "{}",
        missing_key.message
    );

    // Rename, headers, default.
    let renamed = h
        .call(
            "ai.provider_set",
            json!({ "id": "openai-home", "name": "Personal", "headers": { "X-Team": "me" }, "default": true }),
        )
        .await
        .unwrap();
    assert_eq!(renamed["name"], "Personal");
    assert_eq!(renamed["headers"], json!({ "X-Team": "me" }));
    assert_eq!(renamed["is_default"], true);
    let builtin_rename = h
        .call("ai.provider_set", json!({ "id": "openai", "name": "Mine" }))
        .await
        .unwrap_err();
    assert!(builtin_rename.message.contains("built-in"));

    // Kept across a restart.
    drop(h);
    let h = Harness::new(dir.path());
    let personal = h.provider("openai-home").await.unwrap();
    assert_eq!(personal["name"], "Personal");
    assert_eq!(personal["is_default"], true);

    // Remove: the default goes back to automatic; built-ins cannot be removed.
    let removed = h
        .call("ai.provider_remove", json!({ "id": "openai-home" }))
        .await
        .unwrap();
    assert_eq!(removed, json!({ "id": "openai-home", "was_default": true }));
    assert!(h.provider("openai-home").await.is_none());
    let err = h
        .call("ai.provider_remove", json!({ "id": "anthropic" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = h
        .call("ai.provider_remove", json!({ "id": "openai-home" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_provider_is_tested_and_runs_sessions_with_its_headers() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-team", "lab"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "id": "qwen3:8b" }] })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("x-team", "lab"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "Hello" }, "finish_reason": "stop" }]
        })))
        .mount(&server)
        .await;

    let added = h
        .call(
            "ai.provider_add",
            json!({
                "name": "Lab server",
                "template": "ollama",
                "base_url": format!("{}/v1", server.uri()),
                "headers": { "X-Team": "lab" },
            }),
        )
        .await
        .unwrap();
    let id = added["id"].as_str().unwrap().to_string();
    assert_eq!(id, "lab-server");

    let test = h
        .call("ai.provider_test", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(test["ok"], true, "{test}");
    assert_eq!(test["models"], json!(["qwen3:8b"]));
    // The tested list is saved and gives the default model.
    let listed = h.provider(&id).await.unwrap();
    assert_eq!(listed["models"], json!(["qwen3:8b"]));
    assert_eq!(listed["model"], "qwen3:8b");

    let mut rx = h.bus.subscribe();
    let session = h
        .call(
            "ai.session_start",
            json!({ "provider": id, "prompt": "hi" }),
        )
        .await
        .unwrap();
    assert_eq!(session["id"], "lab-server-1");
    assert_eq!(session["provider"], "lab-server");
    let done = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.topic == "ai.done" {
                return event;
            }
        }
    })
    .await
    .expect("no ai.done");
    assert_eq!(done.data["session"], "lab-server-1");
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method.as_str() == "POST"),
        "the model call went to the user's endpoint"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_test_says_what_is_missing_without_a_request() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path());
    h.call(
        "ai.provider_add",
        json!({ "name": "Groq 2", "template": "groq" }),
    )
    .await
    .unwrap();
    let test = h
        .call("ai.provider_test", json!({ "id": "groq-2" }))
        .await
        .unwrap();
    assert_eq!(test["ok"], false);
    assert_eq!(test["error"], "no API key set");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_old_custom_provider_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("ai.json"),
        r#"{ "providers": { "custom": { "base_url": "http://127.0.0.1:9/v1", "model": "m" } },
             "default_provider": "custom" }"#,
    )
    .unwrap();
    let h = Harness::new(dir.path());
    let custom = h.provider("custom").await.expect("migrated");
    assert_eq!(custom["builtin"], false);
    assert_eq!(custom["base_url"], "http://127.0.0.1:9/v1");
    assert_eq!(custom["is_default"], true);
    // The old way of configuring it still works too.
    let set = h
        .call("ai.provider_set", json!({ "id": "custom", "model": "m2" }))
        .await
        .unwrap();
    assert_eq!(set["model"], "m2");
}
