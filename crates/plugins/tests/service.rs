//! The plugin service through the registry, the way the API calls it: plugins (install,
//! palette, enable/disable, tokens), userscripts and webhooks. No script host is needed: plugin
//! processes only start after `start()`, and webhooks go to a stand-in `run.start`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_api::access::PluginAuth;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{Caller, CmdError, Command, Ctx, DataDir, EventBus, Meta, Registry, Toolset};
use bmcp_plugins::{PluginsConfig, PluginsService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What the stand-in `run.start` received.
static RUN_STARTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

struct FakeRunStart;

#[derive(Deserialize, JsonSchema)]
struct RunStartArgs {
    script: String,
    data: Value,
    parallel: Option<u32>,
    retries: Option<u32>,
    heal: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
struct RunStartOutput {
    id: String,
}

impl Command for FakeRunStart {
    const NAME: &'static str = "run.start";
    type Args = RunStartArgs;
    type Output = RunStartOutput;

    fn meta() -> Meta {
        Meta::new("test double", Toolset::Run)
    }

    async fn run(_ctx: Ctx, args: RunStartArgs) -> Result<RunStartOutput, CmdError> {
        let mut starts = RUN_STARTS.lock().unwrap();
        starts.push(json!({ "script": args.script, "data": args.data,
            "parallel": args.parallel, "retries": args.retries, "heal": args.heal }));
        Ok(RunStartOutput {
            id: format!("run-{}", starts.len()),
        })
    }
}

struct Env {
    _dir: tempfile::TempDir,
    data: DataDir,
    service: PluginsService,
    registry: Arc<Registry>,
    backend: Arc<FakeBackend>,
    events: EventBus,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::from_root(dir.path());
    let events = EventBus::new();
    let mut config = PluginsConfig::new(&data, 47800);
    config.host_exe = Some(dir.path().join("no-such-host.exe"));
    let service = PluginsService::open(config, events.clone());
    let mut registry = Registry::new();
    bmcp_plugins::register(&mut registry);
    registry.add::<FakeRunStart>();
    registry.extensions().insert(service.clone());
    let registry = Arc::new(registry);
    service.attach(&registry);
    Env {
        _dir: dir,
        data,
        service,
        registry,
        backend: FakeBackend::with_tabs(&["https://example.com/"]),
        events,
    }
}

impl Env {
    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(Caller::Rest, self.backend.clone(), self.events.clone());
        self.registry.call(ctx, name, args).await
    }
}

fn example_plugin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/examples/plugins/hello-plugin")
}

#[tokio::test]
async fn install_list_palette_enable_remove() {
    let env = env();
    let path = example_plugin();
    let info = env
        .call("plugins.install", json!({ "path": path, "enable": false }))
        .await
        .unwrap();
    assert_eq!(info["name"], "hello-plugin");
    assert_eq!(info["status"], "disabled");
    assert_eq!(info["manifest"]["permissions"], json!(["tabs.list"]));
    assert!(
        env.data
            .root()
            .join("plugins/hello-plugin/main.ts")
            .is_file()
    );

    // Installing again needs replace: true.
    let err = env
        .call("plugins.install", json!({ "path": path }))
        .await
        .unwrap_err();
    assert!(err.message.contains("already installed"), "{err:?}");

    // Palette entries come only from enabled plugins.
    let palette = env.call("plugins.palette", json!({})).await.unwrap();
    assert_eq!(palette["commands"], json!([]));
    let info = env
        .call("plugins.enable", json!({ "name": "hello-plugin" }))
        .await
        .unwrap();
    assert_eq!(info["enabled"], true);
    // Not started yet (the API is "not listening"): it waits, and no token is accepted.
    assert_eq!(info["status"], "waiting");
    assert!(env.service.authenticate("hello-plugin", "").is_none());
    let palette = env.call("plugins.palette", json!({})).await.unwrap();
    assert_eq!(
        palette["commands"],
        json!([{ "plugin": "hello-plugin", "id": "count-tabs", "title": "Hello plugin: count open tabs" }])
    );

    // palette_run sends plugin.command on the bus.
    let mut bus = env.events.subscribe();
    let out = env
        .call(
            "plugins.palette_run",
            json!({ "plugin": "hello-plugin", "id": "count-tabs" }),
        )
        .await
        .unwrap();
    assert_eq!(out["delivered"], false, "not running");
    let event = loop {
        let e = bus.recv().await.unwrap();
        if e.topic == "plugin.command" {
            break e;
        }
    };
    assert_eq!(
        event.data,
        json!({ "plugin": "hello-plugin", "id": "count-tabs" })
    );
    assert!(
        env.call(
            "plugins.palette_run",
            json!({ "plugin": "hello-plugin", "id": "nope" })
        )
        .await
        .is_err()
    );

    // Disabled state survives a restart of the service.
    env.call("plugins.disable", json!({ "name": "hello-plugin" }))
        .await
        .unwrap();
    let reopened = PluginsService::open(PluginsConfig::new(&env.data, 1), EventBus::new());
    let mut registry = Registry::new();
    bmcp_plugins::register(&mut registry);
    registry.extensions().insert(reopened);
    let ctx = Ctx::new(Caller::Rest, env.backend.clone(), env.events.clone());
    let list = registry.call(ctx, "plugins.list", json!({})).await.unwrap();
    assert_eq!(list["plugins"][0]["enabled"], false);

    let out = env
        .call("plugins.remove", json!({ "name": "hello-plugin" }))
        .await
        .unwrap();
    assert_eq!(out["removed"], true);
    let list = env.call("plugins.list", json!({})).await.unwrap();
    assert_eq!(list["plugins"], json!([]));
}

#[tokio::test]
async fn broken_folders_are_listed_with_their_error() {
    let env = env();
    let dir = env.data.root().join("plugins/broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        r#"{"name":"other","version":"1","main":"m.ts"}"#,
    )
    .unwrap();
    std::fs::write(dir.join("m.ts"), "").unwrap();
    let list = env.call("plugins.list", json!({})).await.unwrap();
    let broken = &list["plugins"][0];
    assert_eq!(broken["name"], "broken");
    assert_eq!(broken["status"], "error");
    assert!(
        broken["error"].as_str().unwrap().contains("folder"),
        "{broken}"
    );
    assert!(
        env.call("plugins.enable", json!({ "name": "broken" }))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_missing_script_host_is_reported() {
    let env = env();
    env.call("plugins.install", json!({ "path": example_plugin() }))
        .await
        .unwrap();
    env.service.start("ws://127.0.0.1:1/v1/ws");
    let mut status = Value::Null;
    // Condition wait; the deadline only catches a real hang (busy machines are slow).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let list = env.call("plugins.list", json!({})).await.unwrap();
        status = list["plugins"][0].clone();
        if status["status"] == "error" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(status["status"], "error", "{status}");
    assert!(
        status["error"].as_str().unwrap().contains("script host"),
        "{status}"
    );
    // No process, no token.
    assert!(env.service.authenticate("hello-plugin", "").is_none());
    env.service.shutdown();
}

#[tokio::test]
async fn userscripts_save_list_enable_delete() {
    let env = env();
    let code = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/examples/userscripts/example-banner.ts"),
    )
    .unwrap();
    let saved = env
        .call(
            "userscripts.save",
            json!({ "name": "example-banner", "code": code }),
        )
        .await
        .unwrap();
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["header"]["run_at"], "document-end");
    assert_eq!(
        saved["header"]["matches"],
        json!(["https://example.com/*", "https://*.example.com/*"])
    );

    // A header is required.
    let err = env
        .call(
            "userscripts.save",
            json!({ "name": "x", "code": "console.log(1)" }),
        )
        .await
        .unwrap_err();
    assert!(err.message.contains("@match"), "{err:?}");
    assert!(
        env.call("userscripts.save", json!({ "name": "../x", "code": code }))
            .await
            .is_err()
    );

    env.call(
        "userscripts.enable",
        json!({ "name": "example-banner", "enabled": false }),
    )
    .await
    .unwrap();
    let list = env.call("userscripts.list", json!({})).await.unwrap();
    assert_eq!(list["userscripts"][0]["enabled"], false);
    assert_eq!(list["userscripts"][0]["source"], "user");

    let out = env
        .call("userscripts.delete", json!({ "name": "example-banner" }))
        .await
        .unwrap();
    assert_eq!(out["removed"], true);
    let list = env.call("userscripts.list", json!({})).await.unwrap();
    assert_eq!(list["userscripts"], json!([]));
}

#[tokio::test]
async fn inject_runs_in_an_isolated_world() {
    let env = env();
    env.backend.on_cdp(|_, method, _| match method {
        "Page.getFrameTree" => Some(Ok(json!({ "frameTree": { "frame": { "id": "F1" } } }))),
        "Page.createIsolatedWorld" => Some(Ok(json!({ "executionContextId": 42 }))),
        "Runtime.evaluate" => Some(Ok(
            json!({ "result": { "type": "string", "value": "added" } }),
        )),
        _ => None,
    });
    let out = env
        .call(
            "userscripts.inject",
            json!({ "tab": 1, "js": "return 'added'" }),
        )
        .await
        .unwrap();
    assert_eq!(out, json!({ "tab": 1, "value": "added" }));
    let calls = env.backend.cdp_calls();
    let world = &calls
        .iter()
        .find(|(m, _)| m == "Page.createIsolatedWorld")
        .unwrap()
        .1;
    assert_eq!(world["frameId"], "F1");
    let eval = &calls
        .iter()
        .find(|(m, _)| m == "Runtime.evaluate")
        .unwrap()
        .1;
    assert_eq!(eval["contextId"], 42);
    assert!(
        eval["expression"]
            .as_str()
            .unwrap()
            .contains("return 'added'")
    );
    assert!(!calls.iter().any(|(m, _)| m == "Runtime.enable"));
}

#[tokio::test]
async fn webhooks_check_the_signature_and_start_a_run() {
    let env = env();
    // The hook's script must be a saved script.
    let err = env
        .call("trigger.webhook_set", json!({ "script": "notify" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
    let scripts = env.data.root().join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("notify.ts"), "browser.output(inputs);").unwrap();
    assert!(
        env.call(
            "trigger.webhook_set",
            json!({ "script": "notify", "secret": "short" })
        )
        .await
        .is_err()
    );

    let created = env
        .call(
            "trigger.webhook_set",
            json!({ "id": "n8n-1", "script": "notify", "secret": "0123456789abcdef",
                    "inputs": { "source": "n8n" } }),
        )
        .await
        .unwrap();
    assert_eq!(created["path"], "/v1/hooks/n8n-1");
    assert_eq!(created["secret"], "0123456789abcdef");
    // Listing never shows the secret.
    let list = env.call("trigger.webhook_list", json!({})).await.unwrap();
    assert!(list["hooks"][0].get("secret").is_none());
    assert!(!list.to_string().contains("0123456789abcdef"));

    let body = r#"{"email":"a@b.test"}"#;
    let fire = |signature: String| {
        env.call(
            "trigger.webhook_fire",
            json!({ "id": "n8n-1", "signature": signature, "body": body }),
        )
    };
    let err = fire("sha256=00".into()).await.unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NeedsApproval);
    let err = fire(String::new()).await.unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NeedsApproval);
    assert!(RUN_STARTS.lock().unwrap().is_empty(), "nothing ran");

    let signature = bmcp_plugins::webhooks::signature_for("0123456789abcdef", body.as_bytes());
    let out = fire(signature).await.unwrap();
    assert_eq!(out["script"], "notify");
    let run_id = out["run_id"].as_str().unwrap().to_string();
    let starts = RUN_STARTS.lock().unwrap().clone();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0],
        json!({ "script": "notify", "data": { "rows": [{ "source": "n8n", "email": "a@b.test" }] },
                "parallel": 1, "retries": 0, "heal": false })
    );
    let list = env.call("trigger.webhook_list", json!({})).await.unwrap();
    assert_eq!(list["hooks"][0]["last_run_id"], json!(run_id));

    // Persisted in data/config/webhooks.json.
    let file = std::fs::read_to_string(env.data.config().join("webhooks.json")).unwrap();
    assert!(file.contains("n8n-1") && file.contains("0123456789abcdef"));

    let removed = env
        .call("trigger.webhook_remove", json!({ "id": "n8n-1" }))
        .await
        .unwrap();
    assert_eq!(removed["removed"], true);
}
