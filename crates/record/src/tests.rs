//! Recording sessions against `FakeBackend`: we play the page's part by emitting CDP events
//! (`Runtime.bindingCalled`, `Page.frameNavigated`) and the registry's part by calling real
//! commands or emitting `command.*` events, then check `actions.jsonl` and the CDP calls.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_cdpshare::CdpShare;
use bmcp_core::events::topics;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{BrowserBackend, Caller, CmdError, Ctx, DataDir, EventBus, Registry, TabId};
use serde_json::{Value, json};

use crate::model::Step;
use crate::store::{DataDirExt, now_ms, read_steps};

const URL: &str = "http://127.0.0.1:8000/login.html";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../tests/fixtures/cdp/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A fake tab showing the login fixture, a registry with the browser + record commands, one
/// shared event bus, and a temporary data folder.
struct Harness {
    fake: Arc<FakeBackend>,
    registry: Registry,
    events: EventBus,
    data: tempfile::TempDir,
    /// What the recorder's in-page queue holds (queue mode).
    queue: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    fn new() -> Harness {
        let fake = FakeBackend::with_tabs(&[URL]);
        fake.set_tab(TabId(1), "Sign in", URL, false);
        let queue: Arc<Mutex<Vec<String>>> = Arc::default();
        let tree = fixture("login_form.json");
        let shared = queue.clone();
        fake.on_cdp(move |_tab, method, params| {
            let answer = match method {
                "Page.getFrameTree" => json!({ "frameTree": { "frame": {
                    "id": "F1", "url": URL, "loaderId": "L1" } } }),
                "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "7" }),
                "Page.createIsolatedWorld" => json!({ "executionContextId": 42 }),
                "Accessibility.getFullAXTree" => tree.clone(),
                "Page.getLayoutMetrics" => json!({
                    "cssLayoutViewport": { "clientWidth": 1280, "clientHeight": 720, "pageX": 0, "pageY": 0 },
                    "cssContentSize": { "width": 1280, "height": 900 } }),
                "Runtime.evaluate" => {
                    let expression = params["expression"].as_str().unwrap_or_default();
                    if expression.contains("drain()") {
                        let items: Vec<String> = std::mem::take(&mut *shared.lock().unwrap());
                        json!({ "result": { "type": "object", "value": items } })
                    } else {
                        json!({ "result": { "type": "undefined" } })
                    }
                }
                m if m.starts_with("Page.") || m.starts_with("Runtime.") => json!({}),
                "Network.enable" | "Network.disable" => json!({}),
                _ => return None,
            };
            Some(Ok(answer))
        });
        let mut registry = Registry::new();
        bmcp_browser::register(&mut registry);
        crate::register(&mut registry);
        let data = tempfile::tempdir().unwrap();
        registry
            .extensions()
            .insert(DataDirExt(DataDir::from_root(data.path())));
        Harness {
            fake,
            registry,
            events: EventBus::new(),
            data,
            queue,
        }
    }

    fn ctx(&self) -> Ctx {
        Ctx::new(Caller::Rest, self.fake.clone(), self.events.clone())
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry.call(self.ctx(), name, args).await
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    /// The page reports a recorder event through the binding.
    fn human(&self, event: Value) {
        self.fake.emit_cdp_event(
            TabId(1),
            "Runtime.bindingCalled",
            json!({ "name": "__bmcpRecord", "payload": event.to_string(), "executionContextId": 42 }),
        );
    }

    /// The registry announces an API call (as `Registry::call` does for real commands).
    fn api_started(&self, call_id: u64, name: &str, args: Value) {
        self.events.emit(
            topics::COMMAND_STARTED,
            json!({ "call_id": call_id, "name": name, "caller": {"kind": "ws", "conn": 1},
                    "args": args, "tab": null }),
        );
    }

    fn api_finished(&self, call_id: u64, name: &str, ok: bool) {
        self.events.emit(
            topics::COMMAND_FINISHED,
            json!({ "call_id": call_id, "name": name, "caller": {"kind": "ws", "conn": 1},
                    "duration_ms": 20, "ok": ok, "error": if ok { Value::Null } else { json!("STALE_REF") } }),
        );
    }

    fn methods(&self) -> Vec<String> {
        self.fake.cdp_calls().into_iter().map(|(m, _)| m).collect()
    }

    fn steps(&self, id: &str) -> Vec<Step> {
        read_steps(&self.data.path().join("recordings").join(id)).unwrap()
    }
}

/// The ref the snapshot printed for `line_start`, e.g. `textbox "Email"` → `@1:e5`.
fn ref_in(snapshot: &str, line_start: &str) -> String {
    let line = snapshot
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("- {line_start}")))
        .unwrap_or_else(|| panic!("{line_start} not in\n{snapshot}"));
    line.split_whitespace()
        .find(|w| w.starts_with('@'))
        .unwrap()
        .to_string()
}

/// Let the session task process what was sent (it runs on the same runtime).
async fn settle() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Review item: `net.capture off` used to send `Network.disable` under a running recording's
/// traffic log. The recorder now holds `Network` (and `Page`) through the shared refcount and
/// gives it back on stop; `Network.disable` goes out only after the last holder.
#[tokio::test]
async fn traffic_log_holds_network_through_the_shared_refcount() {
    let h = Harness::new();
    let share = h
        .registry
        .extensions()
        .get_or_insert_with(CdpShare::default);
    let backend: Arc<dyn BrowserBackend> = h.fake.clone();
    // Another user of the tab holds Network too (as `net.capture on` does).
    share
        .acquire(
            &backend,
            TabId(1),
            "Network",
            "net",
            json!({ "maxPostDataSize": 65536 }),
        )
        .await
        .unwrap();
    let out = h.ok("record.start", json!({ "traffic": true })).await;
    let id = out["id"].as_str().unwrap().to_string();
    assert_eq!(
        share.holders(TabId(1), "Network"),
        ["net".to_string(), format!("record:{id}")]
    );
    let disables = || {
        h.methods()
            .iter()
            .filter(|m| *m == "Network.disable")
            .count()
    };

    // `net.capture off`: the recording still needs Network.
    share
        .release(&backend, TabId(1), "Network", "net")
        .await
        .unwrap();
    assert_eq!(disables(), 0);
    // The recording ends: it was the last holder.
    h.ok("record.stop", json!({ "id": id })).await;
    assert_eq!(disables(), 1);
    assert!(share.holders(TabId(1), "Network").is_empty());
    assert!(
        !h.methods().iter().any(|m| m == "Page.disable"),
        "Page is never switched off"
    );
}

#[tokio::test]
async fn start_installs_recorder_in_isolated_world_without_runtime_enable() {
    let h = Harness::new();
    let out = h.ok("record.start", json!({ "name": "Login" })).await;
    let id = out["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("login-"), "{id}");

    let calls = h.fake.cdp_calls();
    let find = |m: &str| {
        calls
            .iter()
            .find(|(name, _)| name == m)
            .unwrap_or_else(|| panic!("{m} not called"))
            .1
            .clone()
    };
    assert_eq!(
        find("Runtime.addBinding"),
        json!({ "name": "__bmcpRecord", "executionContextId": 42 })
    );
    let script = find("Page.addScriptToEvaluateOnNewDocument");
    assert_eq!(script["worldName"], "bmcp-recorder");
    assert!(
        script["source"]
            .as_str()
            .unwrap()
            .contains("let mode = \"binding\"")
    );
    assert_eq!(
        find("Page.createIsolatedWorld"),
        json!({ "frameId": "F1", "worldName": "bmcp-recorder" })
    );
    let eval = calls
        .iter()
        .find(|(m, p)| m == "Runtime.evaluate" && p["contextId"] == 42)
        .expect("recorder evaluated in the isolated world");
    assert!(
        eval.1["expression"]
            .as_str()
            .unwrap()
            .contains("__bmcpRecorder")
    );
    assert!(!h.methods().iter().any(|m| m == "Runtime.enable"));

    let stop = h.ok("record.stop", json!({ "id": id })).await;
    assert!(stop["actions"].as_str().unwrap().ends_with("actions.jsonl"));
    // The recorder is removed again.
    let methods = h.methods();
    assert!(
        methods
            .iter()
            .any(|m| m == "Page.removeScriptToEvaluateOnNewDocument")
    );
    assert!(methods.iter().any(|m| m == "Runtime.removeBinding"));
    let steps = h.steps(&id);
    assert_eq!(steps[0].action, "start");
    assert_eq!(steps[0].url.as_deref(), Some(URL));
}

#[tokio::test]
async fn human_events_and_navigations_are_logged() {
    let h = Harness::new();
    let mut bus = h.events.subscribe();
    let id = h.ok("record.start", json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let t = now_ms();
    h.human(json!({ "type": "hello", "t": t }));
    h.human(json!({ "type": "fill", "t": t + 1, "url": URL, "value": "ann@example.test",
        "target": { "role": "textbox", "name": "Email", "css": "#email", "tag": "input", "inputType": "email" } }));
    h.human(json!({ "type": "fill", "t": t + 2, "url": URL, "value": "***", "secret": true,
        "target": { "role": "textbox", "name": "Password", "css": "#password", "tag": "input", "inputType": "password" } }));
    h.human(json!({ "type": "click", "t": t + 3, "url": URL,
        "target": { "role": "button", "name": "Sign in", "css": "#login > p > button", "tag": "button" } }));
    // An iframe navigation is ignored, the main frame's is logged (a moment after the click).
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.fake.emit_cdp_event(
        TabId(1),
        "Page.frameNavigated",
        json!({ "frame": { "id": "F2", "parentId": "F1", "url": "https://ads.test/" } }),
    );
    h.fake.emit_cdp_event(
        TabId(1),
        "Page.frameNavigated",
        json!({ "frame": { "id": "F1", "url": "http://127.0.0.1:8000/welcome.html" } }),
    );
    // The new document's recorder world gets the binding on the next tick (and its queue,
    // filled before the binding existed, is read once).
    tokio::time::sleep(Duration::from_millis(400)).await;
    let binds = h
        .fake
        .cdp_calls()
        .into_iter()
        .filter(|(m, _)| m == "Runtime.addBinding")
        .count();
    assert_eq!(binds, 2, "bound at start and after the navigation");
    let stop = h.ok("record.stop", json!({})).await;
    assert_eq!(stop["human_capture"], "binding");
    // The Recording panel gets the steps live (`record.*` is a default WS topic).
    let mut topics = Vec::new();
    while let Ok(event) = bus.try_recv() {
        if event.topic.starts_with("record.") {
            topics.push(event.topic);
        }
    }
    assert_eq!(topics.first().map(String::as_str), Some("record.started"));
    assert_eq!(topics.last().map(String::as_str), Some("record.stopped"));
    assert_eq!(topics.iter().filter(|t| *t == "record.step").count(), 4);

    let steps = h.steps(&id);
    let actions: Vec<&str> = steps.iter().map(|s| s.action.as_str()).collect();
    assert_eq!(actions, ["start", "fill", "fill", "click", "navigated"]);
    assert!(steps[2].secret && steps[2].value.as_deref() == Some("***"));
    assert_eq!(
        steps[4].url.as_deref(),
        Some("http://127.0.0.1:8000/welcome.html")
    );
    assert!(steps.iter().all(|s| s.dup_of.is_none()));

    // And the script: the click loaded a page, so it waits instead of loading it again.
    let code = h
        .ok("record.codegen", json!({ "id": id, "parametrize": true }))
        .await;
    let code = code["code"].as_str().unwrap();
    assert!(
        code.contains(
            r#"await act("textbox", "Email", "fill", inputs.email ?? "ann@example.test");"#
        ),
        "{code}"
    );
    assert!(code.contains(r#"await act("textbox", "Password", "fill", need("password"));"#));
    assert!(code.contains(r#"await act("button", "Sign in", "click");"#));
    assert!(code.contains(
        r#"await browser.nav.wait({ for: "load" }); // → http://127.0.0.1:8000/welcome.html"#
    ));
    let path = h.data.path().join("scripts").join("recording.ts");
    assert_eq!(std::fs::read_to_string(path).unwrap(), code);
}

#[tokio::test]
async fn api_actions_get_locators_and_their_human_copies_are_deduplicated() {
    let h = Harness::new();
    let id = h.ok("record.start", json!({ "name": "login" })).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A real snapshot through the registry: logged as a (read-only) command, and it is what
    // makes the refs mean something.
    let snap = h.ok("page.snapshot", json!({})).await;
    let snap = snap["text"].as_str().unwrap();
    let email = ref_in(snap, "textbox \"Email\"");
    let password = ref_in(snap, "textbox \"Password\"");
    let sign_in = ref_in(snap, "button \"Sign in\"");

    // page.act fill: while it runs the page sees trusted typing → recorder reports a fill.
    h.api_started(
        101,
        "page.act",
        json!({ "ref": email, "action": "fill", "value": "ann@example.test" }),
    );
    settle().await;
    h.human(json!({ "type": "fill", "t": now_ms(), "value": "ann@example.test",
        "target": { "role": "textbox", "name": "Email", "css": "#email", "tag": "input", "inputType": "email" } }));
    settle().await;
    h.api_finished(101, "page.act", true);

    // The password: the API args hold it in clear text until we redact them.
    h.api_started(
        102,
        "page.act",
        json!({ "ref": password, "action": "fill", "value": "hunter2" }),
    );
    settle().await;
    h.api_finished(102, "page.act", true);
    h.api_started(
        103,
        "page.act",
        json!({ "ref": sign_in, "action": "click" }),
    );
    settle().await;
    h.human(json!({ "type": "click", "t": now_ms(),
        "target": { "role": "button", "name": "Sign in", "css": "#login > p > button", "tag": "button" } }));
    settle().await;
    h.api_finished(103, "page.act", true);
    // A failed call is kept in the log but not in the script.
    h.api_started(
        104,
        "page.act",
        json!({ "ref": "@1:e99", "action": "click" }),
    );
    h.api_finished(104, "page.act", false);
    h.api_started(
        105,
        "nav.wait",
        json!({ "for": "text", "value": "Welcome" }),
    );
    h.api_finished(105, "nav.wait", true);
    // Long after the API calls: a person clicks something.
    h.human(json!({ "type": "click", "t": now_ms() + 10_000,
        "target": { "role": "link", "name": "Home", "css": "header > a", "tag": "a" } }));
    settle().await;
    h.ok("record.stop", json!({ "id": id })).await;

    let steps = h.steps(&id);
    let text = std::fs::read_to_string(
        h.data
            .path()
            .join("recordings")
            .join(&id)
            .join("actions.jsonl"),
    )
    .unwrap();
    assert!(!text.contains("hunter2"), "password leaked:\n{text}");

    let api_fill = steps
        .iter()
        .find(|s| s.command.as_ref().is_some_and(|c| c.call_id == 101))
        .unwrap();
    let target = api_fill.target.as_ref().unwrap();
    assert_eq!(
        (target.role.as_deref(), target.name.as_deref()),
        (Some("textbox"), Some("Email"))
    );
    assert_eq!(target.from.as_deref(), Some("snapshot"));
    // Merged from the recorder's copy:
    assert_eq!(target.css.as_deref(), Some("#email"), "{text}");
    let dup = steps
        .iter()
        .find(|s| s.source == crate::model::Source::Human && s.action == "fill")
        .unwrap();
    assert_eq!(dup.dup_of, Some(api_fill.seq));

    let api_pwd = steps
        .iter()
        .find(|s| s.command.as_ref().is_some_and(|c| c.call_id == 102))
        .unwrap();
    assert!(api_pwd.secret);
    assert_eq!(api_pwd.value.as_deref(), Some("***"));
    assert_eq!(api_pwd.command.as_ref().unwrap().args["value"], "***");

    let human_click = steps
        .iter()
        .find(|s| {
            s.action == "click"
                && s.source == crate::model::Source::Human
                && s.target.as_ref().unwrap().name.as_deref() == Some("Sign in")
        })
        .unwrap();
    assert!(human_click.dup_of.is_some());
    let home = steps
        .iter()
        .find(|s| s.target.as_ref().and_then(|t| t.name.as_deref()) == Some("Home"))
        .unwrap();
    assert_eq!(home.dup_of, None);

    let code = h
        .ok("record.codegen", json!({ "id": id, "parametrize": true }))
        .await;
    let code = code["code"].as_str().unwrap();
    let body: Vec<&str> = code
        .lines()
        .skip_while(|l| !l.starts_with("// ---- steps"))
        .skip(1)
        .collect();
    assert_eq!(
        body,
        [
            r#"await browser.nav.goto({ url: "http://127.0.0.1:8000/login.html" });"#,
            r#"await act("textbox", "Email", "fill", inputs.email ?? "ann@example.test");"#,
            r#"await act("textbox", "Password", "fill", need("password"));"#,
            r#"await act("button", "Sign in", "click");"#,
            r#"await browser.nav.wait({ for: "text", value: "Welcome" });"#,
            r#"await act("link", "Home", "click");"#,
            r#"console.log("done: login");"#,
        ]
    );

    let summary = h.ok("record.get", json!({ "id": id })).await;
    let summary = summary["text"].as_str().unwrap();
    assert!(
        summary.contains("api fill textbox \"Email\" = \"ann@example.test\""),
        "{summary}"
    );
    assert!(summary.contains("FAILED STALE_REF"), "{summary}");
    assert!(summary.contains("(dup of #"), "{summary}");
}

#[tokio::test(start_paused = true)]
async fn without_binding_events_it_falls_back_to_polling() {
    let h = Harness::new();
    let id = h.ok("record.start", json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // No bindingCalled ever arrives; the page queues instead.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let reinstalled: Vec<_> = h
        .fake
        .cdp_calls()
        .into_iter()
        .filter(|(m, _)| m == "Page.addScriptToEvaluateOnNewDocument")
        .collect();
    assert_eq!(reinstalled.len(), 2, "binding source, then queue source");
    assert!(
        reinstalled[1].1["source"]
            .as_str()
            .unwrap()
            .contains("let mode = \"queue\"")
    );
    h.queue.lock().unwrap().push(
        json!({ "type": "click", "t": now_ms(), "url": URL,
            "target": { "role": "checkbox", "name": "Remember me" } })
        .to_string(),
    );
    h.queue.lock().unwrap().push(
        json!({ "type": "key", "t": now_ms() + 1, "url": URL, "key": "Enter", "target": null })
            .to_string(),
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    let stop = h.ok("record.stop", json!({})).await;
    assert_eq!(stop["human_capture"], "poll");
    let steps = h.steps(&id);
    let actions: Vec<&str> = steps.iter().map(|s| s.action.as_str()).collect();
    assert_eq!(actions, ["start", "click", "press"]);
    let drains = h
        .fake
        .cdp_calls()
        .into_iter()
        .filter(|(m, p)| {
            m == "Runtime.evaluate" && p["expression"].as_str().unwrap().contains("drain()")
        })
        .count();
    assert!(drains >= 3, "polled every 250 ms: {drains}");
}

#[tokio::test]
async fn record_list_get_and_errors() {
    let h = Harness::new();
    let id = h.ok("record.start", json!({ "name": "one" })).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let list = h.ok("record.list", json!({})).await;
    assert_eq!(list["recordings"][0]["id"], id.as_str());
    assert_eq!(list["recordings"][0]["active"], true);
    h.ok("record.stop", json!({})).await;
    let list = h.ok("record.list", json!({})).await;
    assert_eq!(list["recordings"][0]["active"], false);
    let jsonl = h
        .ok("record.get", json!({ "id": id, "format": "jsonl" }))
        .await;
    assert!(jsonl["text"].as_str().unwrap().starts_with(r#"{"seq":1,"#));

    let err = h.call("record.stop", json!({})).await.unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
    let err = h
        .call("record.get", json!({ "id": "../etc" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
}

#[tokio::test]
async fn recording_with_video_uses_the_encoder_and_acks_frames() {
    use base64::Engine;
    let h = Harness::new();
    // Fake encoder: this test binary copying stdin to a file (see video::tests).
    h.registry
        .extensions()
        .insert(crate::EncoderOverride(Box::new(|out| {
            Box::new(move |_, _| {
                Ok(crate::video::Encoder {
                    program: std::env::current_exe().unwrap(),
                    args: vec![
                        "video::tests::fake_encoder_process".into(),
                        "--exact".into(),
                    ],
                    env: vec![("FAKE_ENCODER_OUT".into(), out.display().to_string())],
                    codec: "fake".into(),
                })
            })
        })));
    let out = h
        .ok("record.start", json!({ "name": "vid", "video": true }))
        .await;
    let video = out["video"].as_str().unwrap().to_string();
    // A minimal JPEG header (80×64).
    let mut jpeg = vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x40, 0x00, 0x50,
    ];
    jpeg.extend([0; 12]);
    let data = base64::engine::general_purpose::STANDARD.encode(&jpeg);
    for session in [11, 12] {
        h.fake.emit_cdp_event(
            TabId(1),
            "Page.screencastFrame",
            json!({ "data": data, "sessionId": session }),
        );
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let stop = h.ok("record.stop", json!({})).await;
    assert_eq!(stop["video"].as_str(), Some(video.as_str()));
    let acks: Vec<i64> = h
        .fake
        .cdp_calls()
        .into_iter()
        .filter(|(m, _)| m == "Page.screencastFrameAck")
        .map(|(_, p)| p["sessionId"].as_i64().unwrap())
        .collect();
    assert_eq!(acks, [11, 12]);
    // At least the first frame (more when the encoder process started quickly).
    let bytes = std::fs::metadata(&video).unwrap().len() as usize;
    assert!(bytes >= jpeg.len(), "{bytes} bytes");
    assert_eq!(bytes % jpeg.len(), 0, "whole frames only");
}

#[tokio::test]
async fn video_without_ffmpeg_or_bad_args_is_a_clear_error() {
    let h = Harness::new();
    let err = h
        .call("record.video_start", json!({ "fps": 0 }))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    let err = h.call("record.video_stop", json!({})).await.unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
    // The missing-ffmpeg error names the fix.
    let missing = crate::video::ffmpeg_missing();
    assert!(missing.hint.unwrap().contains("winget install Gyan.FFmpeg"));
}
