//! `FakeBackend` tests: the CDP calls hints make, and what typing in the page leads to.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{Caller, Ctx, Event, Registry, TabId};
use serde_json::{Value, json};

use crate::service::{BINDING, HintsService, PICKED_EVENT, WORLD};

const TAB: TabId = TabId(1);
/// Backend node id of every element in the fake page.
const NODE: i64 = 42;

/// A fake page with two interactive elements: a link and a button. `context` is the id
/// `Page.createIsolatedWorld` hands out (bump it to simulate a new document).
fn fake_page(context: Arc<AtomicI64>) -> Arc<FakeBackend> {
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);
    let scripts = AtomicI64::new(0);
    fake.on_cdp(move |_, method, params| {
        let value = |v: Value| Some(Ok(json!({ "result": { "type": "object", "value": v } })));
        match method {
            "Page.enable"
            | "Runtime.addBinding"
            | "Runtime.releaseObject"
            | "Page.removeScriptToEvaluateOnNewDocument"
            | "DOM.scrollIntoViewIfNeeded"
            | "DOM.focus"
            | "Input.dispatchMouseEvent" => Some(Ok(json!({}))),
            "Page.addScriptToEvaluateOnNewDocument" => {
                let n = scripts.fetch_add(1, Ordering::SeqCst) + 1;
                Some(Ok(json!({ "identifier": format!("script-{n}") })))
            }
            "Page.getFrameTree" => Some(Ok(
                json!({ "frameTree": { "frame": { "id": "main", "loaderId": "l1" } } }),
            )),
            "Page.createIsolatedWorld" => Some(Ok(
                json!({ "executionContextId": context.load(Ordering::SeqCst) }),
            )),
            "Runtime.evaluate" => {
                let expression = params["expression"].as_str().unwrap_or_default();
                if expression.contains("__bmcpHints.collect()") {
                    value(json!({ "ms": 1.25, "items": [
                        { "tag": "a", "text": "About", "href": "https://a.test/about" },
                        { "tag": "button", "text": "Go" },
                    ]}))
                } else if expression.contains("__bmcpHints.render(") {
                    value(json!(2))
                } else if expression.contains("__bmcpHints.element(") {
                    Some(Ok(json!({ "result": { "type": "object", "objectId": "hint-el" } })))
                } else if expression.contains("__bmcpHints.copy(") {
                    value(json!("clipboard_api"))
                } else if expression.contains("visibilityState") {
                    value(json!("visible"))
                } else if expression.contains(".drain()") {
                    value(json!([]))
                } else {
                    value(Value::Null)
                }
            }
            "DOM.describeNode" => Some(Ok(json!({ "node": { "backendNodeId": NODE } }))),
            "DOM.resolveNode" => Some(Ok(json!({ "object": { "objectId": "obj-42" } }))),
            "Runtime.callFunctionOn" => {
                let function = params["functionDeclaration"].as_str().unwrap_or_default();
                if function.contains("isConnected") {
                    value(json!({ "connected": true, "tag": "button" }))
                } else {
                    value(json!(true))
                }
            }
            "DOM.getContentQuads" => Some(Ok(
                json!({ "quads": [[100, 100, 200, 100, 200, 120, 100, 120]] }),
            )),
            "Page.getLayoutMetrics" => Some(Ok(json!({
                "cssLayoutViewport": { "clientWidth": 1280, "clientHeight": 720, "pageX": 0, "pageY": 0 },
                "cssContentSize": { "width": 1280, "height": 720 },
            }))),
            "DOM.getNodeForLocation" => Some(Ok(json!({ "backendNodeId": NODE }))),
            _ => None,
        }
    });
    fake
}

struct Env {
    fake: Arc<FakeBackend>,
    registry: Registry,
    context: Arc<AtomicI64>,
    /// A context with the registry's extensions (for calling the service directly).
    ctx: Ctx,
}

fn env() -> Env {
    let context = Arc::new(AtomicI64::new(7));
    let fake = fake_page(context.clone());
    let mut registry = Registry::new();
    crate::register(&mut registry);
    let mut c = ctx(Caller::Rest, fake.clone());
    c.extensions = registry.extensions().clone();
    Env {
        fake,
        registry,
        context,
        ctx: c,
    }
}

impl Env {
    async fn call(&self, name: &str, args: Value) -> Value {
        self.registry
            .call(self.ctx.clone(), name, args)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn methods(&self) -> Vec<String> {
        self.fake.cdp_calls().into_iter().map(|(m, _)| m).collect()
    }

    /// Index of the first CDP call matching `pred`.
    fn position(&self, pred: impl Fn(&str, &Value) -> bool) -> Option<usize> {
        self.fake.cdp_calls().iter().position(|(m, p)| pred(m, p))
    }

    /// The page script reports `payload` through the binding.
    fn page_says(&self, payload: Value) {
        self.fake.emit_cdp_event(
            TAB,
            "Runtime.bindingCalled",
            json!({ "name": BINDING, "payload": payload.to_string(), "executionContextId": 7 }),
        );
    }

    /// Wait (real time, up to 2 s) until a CDP call matches `pred`.
    async fn wait_for(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> usize {
        for _ in 0..400 {
            if let Some(i) = self.position(&pred) {
                return i;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no {what}; calls: {:?}", self.methods());
    }
}

fn evaluates(expression_part: &str) -> impl Fn(&str, &Value) -> bool + '_ {
    move |m, p| {
        m == "Runtime.evaluate"
            && p["expression"]
                .as_str()
                .is_some_and(|e| e.contains(expression_part))
    }
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<Event>, topic: &str) -> Event {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = rx.recv().await.expect("bus open");
            if event.topic == topic {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {topic} event"))
}

#[tokio::test]
async fn show_installs_in_an_isolated_world_without_runtime_enable() {
    let env = env();
    let out = env.call("hints.show", json!({ "tab": 1 })).await;
    assert_eq!(out["count"], 2);
    assert_eq!(out["mode"], "click");
    assert_eq!(out["hints"][0]["label"], "a");
    assert_eq!(out["hints"][0]["href"], "https://a.test/about");
    assert_eq!(out["hints"][1]["label"], "s");
    assert_eq!(out["page_ms"], 1.3);

    // Subscribed to the tab's events before enabling anything.
    let calls = env.fake.calls();
    let subscribed = calls.iter().position(|c| c == "cdp_events 1").unwrap();
    let enabled = calls.iter().position(|c| c == "cdp 1 Page.enable").unwrap();
    assert!(subscribed < enabled, "{calls:?}");

    assert_eq!(
        env.methods(),
        [
            "Page.enable",
            "Page.addScriptToEvaluateOnNewDocument",
            "Page.getFrameTree",
            "Page.createIsolatedWorld",
            "Runtime.addBinding",
            "Runtime.evaluate", // the script itself + drain
            "Runtime.evaluate", // collect
            "Runtime.evaluate", // render
        ]
    );
    let calls = env.fake.cdp_calls();
    assert_eq!(calls[1].1["worldName"], WORLD);
    let source = calls[1].1["source"].as_str().unwrap();
    assert!(
        source.contains(r#""keys":{"click":"f","newtab":"F"}"#),
        "config baked in"
    );
    assert!(!source.contains("__CONFIG__"));
    assert_eq!(calls[3].1["worldName"], WORLD);
    assert_eq!(
        calls[4].1,
        json!({ "name": BINDING, "executionContextId": 7 })
    );
    for (_, params) in &calls[5..] {
        assert_eq!(params["contextId"], 7, "evaluates run in our world");
    }
    assert!(
        calls[7].1["expression"]
            .as_str()
            .unwrap()
            .contains(r#"render(["a","s"])"#)
    );
    assert!(!env.methods().iter().any(|m| m == "Runtime.enable"));

    // A second show reuses the installation.
    env.call("hints.show", json!({ "tab": 1 })).await;
    let installs = env
        .methods()
        .iter()
        .filter(|m| *m == "Page.addScriptToEvaluateOnNewDocument")
        .count();
    assert_eq!(installs, 1);
}

#[tokio::test]
async fn typing_a_label_clicks_with_trusted_input() {
    let env = env();
    let mut bus = env.ctx.events.subscribe();
    env.call("hints.show", json!({ "tab": 1 })).await;
    env.page_says(json!({ "type": "key", "key": "s" }));

    let pressed = env
        .wait_for("mouse press", |m, p| {
            m == "Input.dispatchMouseEvent" && p["type"] == "mousePressed"
        })
        .await;
    let hide = env.position(evaluates("__bmcpHints.hide()")).unwrap();
    let element = env.position(evaluates("__bmcpHints.element(1)")).unwrap();
    let describe = env.position(|m, _| m == "DOM.describeNode").unwrap();
    let hit_test = env.position(|m, _| m == "DOM.getNodeForLocation").unwrap();
    // Labels off, then the element, then page.act's checks, then the click.
    assert!(hide < element && element < describe && describe < hit_test && hit_test < pressed);
    let (_, params) = &env.fake.cdp_calls()[describe];
    assert_eq!(params["objectId"], "hint-el");

    let picked = next_event(&mut bus, PICKED_EVENT).await;
    assert_eq!(picked.data["ok"], true, "{:?}", picked.data);
    assert_eq!(picked.data["label"], "s");
    assert_eq!(picked.data["tag"], "button");
    assert!(
        picked.data["result"]
            .as_str()
            .unwrap()
            .starts_with("clicked hint s button \"Go\"")
    );
}

#[tokio::test]
async fn newtab_mode_opens_the_link_in_a_background_tab() {
    let env = env();
    let mut bus = env.ctx.events.subscribe();
    env.call("hints.show", json!({ "tab": 1, "mode": "newtab" }))
        .await;
    env.page_says(json!({ "type": "key", "key": "A" })); // Shift held: still "a"
    let picked = next_event(&mut bus, PICKED_EVENT).await;
    assert_eq!(picked.data["ok"], true, "{:?}", picked.data);
    assert!(
        env.fake
            .calls()
            .contains(&"open_tab https://a.test/about background".to_string())
    );
    assert!(
        !env.methods()
            .iter()
            .any(|m| m == "Input.dispatchMouseEvent")
    );
}

#[tokio::test]
async fn copy_mode_writes_the_link_with_a_user_gesture() {
    let env = env();
    let mut bus = env.ctx.events.subscribe();
    env.call("hints.show", json!({ "tab": 1, "mode": "copy" }))
        .await;
    env.page_says(json!({ "type": "key", "key": "a" }));
    let picked = next_event(&mut bus, PICKED_EVENT).await;
    assert_eq!(picked.data["ok"], true, "{:?}", picked.data);
    let copy = env.position(evaluates("__bmcpHints.copy(")).unwrap();
    let (_, params) = &env.fake.cdp_calls()[copy];
    assert_eq!(params["userGesture"], true);
    assert!(
        params["expression"]
            .as_str()
            .unwrap()
            .contains(r#"copy("https://a.test/about")"#)
    );
}

#[tokio::test]
async fn typos_escape_and_keys_after_cancel_do_nothing() {
    let env = env();
    env.call("hints.show", json!({ "tab": 1 })).await;
    let before = env.methods().len();
    env.page_says(json!({ "type": "key", "key": "x" })); // no label starts with x
    env.page_says(json!({ "type": "key", "key": "Escape" }));
    env.wait_for("hide", evaluates("__bmcpHints.hide()")).await;
    // Only the hide was evaluated for these two keys.
    assert_eq!(env.methods().len(), before + 1, "{:?}", env.methods());

    // After the cancel a label key is not a pick any more.
    env.page_says(json!({ "type": "key", "key": "a" }));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(env.position(|m, _| m == "DOM.describeNode").is_none());
}

#[tokio::test]
async fn the_trigger_key_in_the_page_shows_hints() {
    let env = env();
    let service = HintsService::from_extensions(&env.ctx.extensions);
    service.ensure(&env.ctx, TAB).await.unwrap();
    assert!(env.position(evaluates("__bmcpHints.collect()")).is_none());

    env.page_says(json!({ "type": "trigger", "mode": "newtab" }));
    env.wait_for("render", evaluates("__bmcpHints.render("))
        .await;
    // Keys typed right after the trigger are handled after the hints are up.
    env.page_says(json!({ "type": "key", "key": "a" }));
    for _ in 0..400 {
        if env
            .fake
            .calls()
            .contains(&"open_tab https://a.test/about background".to_string())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no new tab: {:?}", env.fake.calls());
}

#[tokio::test]
async fn a_new_document_gets_the_binding_again() {
    let env = env();
    env.call("hints.show", json!({ "tab": 1 })).await;
    env.context.store(8, Ordering::SeqCst);
    env.fake.emit_cdp_event(
        TAB,
        "Page.frameNavigated",
        json!({ "frame": { "id": "main", "url": "https://a.test/next" } }),
    );
    env.wait_for("rebind", |m, p| {
        m == "Runtime.addBinding" && p["executionContextId"] == 8
    })
    .await;
    // An iframe navigating changes nothing.
    env.fake.emit_cdp_event(
        TAB,
        "Page.frameNavigated",
        json!({ "frame": { "id": "child", "parentId": "main", "url": "https://ads.test/" } }),
    );
    // The old hints went with the old document: a label key is no pick.
    env.page_says(json!({ "type": "key", "key": "a" }));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(env.position(|m, _| m == "DOM.describeNode").is_none());
    let bindings = env
        .methods()
        .iter()
        .filter(|m| *m == "Runtime.addBinding")
        .count();
    assert_eq!(bindings, 2);
}

#[tokio::test]
async fn new_settings_replace_the_new_document_script() {
    let env = env();
    env.call("hints.show", json!({ "tab": 1 })).await;
    let out = env
        .call(
            "hints.config",
            json!({ "keys": { "click": "g" }, "disable_site": "news.test" }),
        )
        .await;
    assert_eq!(out["config"]["keys"]["click"], "g");
    assert_eq!(out["config"]["disabled_sites"], json!(["news.test"]));

    let calls = env.fake.cdp_calls();
    let removed = calls
        .iter()
        .find(|(m, _)| m == "Page.removeScriptToEvaluateOnNewDocument")
        .unwrap();
    assert_eq!(removed.1["identifier"], "script-1");
    let added: Vec<&Value> = calls
        .iter()
        .filter(|(m, _)| m == "Page.addScriptToEvaluateOnNewDocument")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(added.len(), 2);
    let source = added[1]["source"].as_str().unwrap();
    assert!(source.contains(r#""click":"g""#) && source.contains("news.test"));
    assert!(env.position(evaluates("__bmcpHints.configure(")).is_some());

    // Reading the settings changes nothing.
    let count = env.methods().len();
    env.call("hints.config", json!({})).await;
    assert_eq!(env.methods().len(), count);
}

#[tokio::test]
async fn hide_and_clipboard_commands() {
    let env = env();
    // Nothing installed yet: nothing to hide, no CDP calls.
    let out = env.call("hints.hide", json!({ "tab": 1 })).await;
    assert_eq!(out["hidden"], false);
    assert!(env.methods().is_empty());

    env.call("hints.show", json!({ "tab": 1 })).await;
    let out = env.call("hints.hide", json!({ "tab": 1 })).await;
    assert_eq!(out["hidden"], true);

    let out = env
        .call("clipboard.write", json!({ "tab": 1, "text": "hello" }))
        .await;
    assert_eq!(out["method"], "clipboard_api");
}

#[tokio::test]
async fn closing_the_tab_forgets_it() {
    let env = env();
    env.call("hints.show", json!({ "tab": 1 })).await;
    let service = HintsService::from_extensions(&env.ctx.extensions);
    assert_eq!(service.tabs().await, [TAB]);
    service.forget(TAB).await;
    assert!(service.tabs().await.is_empty());
}
