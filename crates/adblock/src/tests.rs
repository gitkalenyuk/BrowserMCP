//! Command tests against `FakeBackend`: the CDP call sequences of the interim `Fetch`
//! interception, policies end to end, persistence and reader mode plumbing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{BackendError, Caller, CmdError, ErrorCode, Registry, TabId, Toolset};
use serde_json::{Value, json};

use crate::AdblockService;
use crate::interim::WORLD;

const RULES: &str = "\
||ads.example^
news.test##.sponsored
##.ad-banner
";

struct Env {
    fake: Arc<FakeBackend>,
    registry: Registry,
    service: Arc<AdblockService>,
    _tmp: tempfile::TempDir,
}

/// A browser with one tab on https://news.test/ and an engine built from [`RULES`].
/// `fail_block` makes `Fetch.failRequest` fail (to test the fallback).
fn env_with(fail_block: Arc<AtomicBool>) -> Env {
    build(fail_block, false)
}

/// Like [`env`], with the `net.*` commands registered too.
fn env_with_net() -> Env {
    build(Arc::new(AtomicBool::new(false)), true)
}

fn build(fail_block: Arc<AtomicBool>, with_net: bool) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeBackend::with_tabs(&["https://news.test/"]);
    fake.on_cdp(move |_tab, method, _params| {
        let reply = match method {
            "Page.getFrameTree" => {
                json!({ "frameTree": { "frame": { "id": "MAIN", "url": "https://news.test/" } } })
            }
            "Page.createIsolatedWorld" => json!({ "executionContextId": 42 }),
            "Page.addScriptToEvaluateOnNewDocument" => json!({ "identifier": "s1" }),
            // The page uses the class "ad-banner" (matches the generic rule).
            "Runtime.evaluate" => {
                json!({ "result": { "type": "object", "value": { "classes": ["ad-banner", "x"], "ids": [] } } })
            }
            "Fetch.failRequest" if fail_block.load(Ordering::Relaxed) => {
                return Some(Err(BackendError::Cdp("Invalid InterceptionId.".into())));
            }
            _ => json!({}),
        };
        Some(Ok(reply))
    });
    let mut registry = Registry::new();
    crate::register(&mut registry);
    if with_net {
        bmcp_net::register(&mut registry);
    }
    let service = registry
        .extensions()
        .insert(AdblockService::new(tmp.path()));
    service.load_rules(&[RULES]);
    Env {
        fake,
        registry,
        service,
        _tmp: tmp,
    }
}

fn env() -> Env {
    env_with(Arc::new(AtomicBool::new(false)))
}

impl Env {
    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry
            .call(ctx(Caller::Rest, self.fake.clone()), name, args)
            .await
    }

    /// Methods of all CDP calls so far.
    fn methods(&self) -> Vec<String> {
        self.fake.cdp_calls().into_iter().map(|(m, _)| m).collect()
    }

    /// Wait (real time, up to 2 s) until a CDP call matches.
    async fn wait_call(&self, method: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..400 {
            if let Some((_, params)) = self
                .fake
                .cdp_calls()
                .into_iter()
                .find(|(m, p)| m == method && pred(p))
            {
                return params;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no {method} call; calls: {:?}", self.methods());
    }

    /// The answers (`Fetch.*Request`, `continueResponse`) sent for one request id.
    fn answers_for(&self, id: &str) -> Vec<String> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, p)| {
                p["requestId"] == id
                    && matches!(
                        m.as_str(),
                        "Fetch.continueRequest"
                            | "Fetch.continueResponse"
                            | "Fetch.fulfillRequest"
                            | "Fetch.failRequest"
                    )
            })
            .map(|(m, _)| m)
            .collect()
    }

    /// `patterns` of every `Fetch.enable` so far.
    fn fetch_enables(&self) -> Vec<Value> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, _)| m == "Fetch.enable")
            .map(|(_, p)| p["patterns"].clone())
            .collect()
    }

    fn paused(&self, id: &str, url: &str, resource_type: &str, frame: &str) {
        self.fake.emit_cdp_event(
            TabId(1),
            "Fetch.requestPaused",
            json!({
                "requestId": id,
                "request": { "url": url, "method": "GET", "headers": {} },
                "frameId": frame,
                "resourceType": resource_type,
            }),
        );
    }
}

fn index_of(methods: &[String], method: &str) -> usize {
    methods
        .iter()
        .position(|m| m == method)
        .unwrap_or_else(|| panic!("{method} not called: {methods:?}"))
}

fn has_id(id: &'static str) -> impl Fn(&Value) -> bool {
    move |p: &Value| p["requestId"] == id
}

#[tokio::test]
async fn apply_enables_fetch_and_injects_cosmetics() {
    let env = env();
    assert!(env.service.gate.has_engine());
    let out = env
        .call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    assert_eq!(out["engine"]["loaded"], true);
    assert_eq!(out["tab"], 1);
    assert_eq!(out["on"], true);
    assert_eq!(out["report"]["generic_selectors"], 1);
    assert!(out["report"]["hidden_selectors"].as_u64().unwrap() >= 2);

    // Subscribed to events before Fetch was enabled (nothing can be missed).
    let calls = env.fake.calls();
    let subscribed = calls.iter().position(|c| c == "cdp_events 1").unwrap();
    let enabled = calls
        .iter()
        .position(|c| c == "cdp 1 Fetch.enable")
        .unwrap();
    assert!(subscribed < enabled, "{calls:?}");

    let cdp = env.fake.cdp_calls();
    let fetch = &cdp.iter().find(|(m, _)| m == "Fetch.enable").unwrap().1;
    assert_eq!(
        fetch["patterns"],
        json!([{ "urlPattern": "*", "requestStage": "Request" }])
    );
    let script = &cdp
        .iter()
        .find(|(m, _)| m == "Page.addScriptToEvaluateOnNewDocument")
        .unwrap()
        .1;
    assert_eq!(script["worldName"], WORLD);
    let source = script["source"].as_str().unwrap();
    assert!(source.contains(".sponsored{display:none!important}"));
    assert!(source.contains("\"news.test\""));
    // The current page gets the CSS through an isolated world, never DOM.* or Runtime.enable.
    let world = &cdp
        .iter()
        .find(|(m, _)| m == "Page.createIsolatedWorld")
        .unwrap()
        .1;
    assert_eq!(world["frameId"], "MAIN");
    let evals: Vec<&Value> = cdp
        .iter()
        .filter(|(m, _)| m == "Runtime.evaluate")
        .map(|(_, p)| p)
        .collect();
    assert!(evals.iter().all(|p| p["contextId"] == 42));
    assert!(evals.iter().any(|p| {
        p["expression"]
            .as_str()
            .unwrap()
            .contains(".ad-banner{display")
    }));
    assert!(
        !env.methods()
            .iter()
            .any(|m| m.starts_with("DOM.") || m == "Runtime.enable")
    );

    // A second apply is a no-op.
    let again = env
        .call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    assert_eq!(again["already"], true);
}

#[tokio::test]
async fn paused_requests_are_failed_or_continued() {
    let env = env();
    env.call("adblock.apply", json!({ "tab": 1, "on": true }))
        .await
        .unwrap();
    env.paused("r1", "https://ads.example/banner.js", "Script", "MAIN");
    env.paused("r2", "https://news.test/app.js", "Script", "MAIN");
    let failed = env.wait_call("Fetch.failRequest", has_id("r1")).await;
    assert_eq!(failed["errorReason"], "BlockedByClient");
    env.wait_call("Fetch.continueRequest", has_id("r2")).await;
    // Each request is answered exactly once.
    let answers = env
        .fake
        .cdp_calls()
        .iter()
        .filter(|(m, _)| m.starts_with("Fetch.") && m.ends_with("Request"))
        .count();
    assert_eq!(answers, 2);

    let stats = env
        .call("adblock.stats", json!({ "tab": 1 }))
        .await
        .unwrap();
    let tab = &stats["tabs"][0];
    assert_eq!(tab["requests"], 2);
    assert_eq!(tab["blocked"], 1);
    assert_eq!(tab["blocked_hosts"]["ads.example"], 1);
    assert_eq!(tab["interception"]["paused"], 2);
    assert_eq!(tab["interception"]["blocked"], 1);
    assert_eq!(tab["interception"]["continued"], 1);
    assert_eq!(tab["interception"]["active"], true);
}

#[tokio::test]
async fn site_policy_blocks_by_type_and_third_party() {
    let env = env();
    env.call(
        "simplify.site",
        json!({ "host": "news.test", "images": false, "third_party": false }),
    )
    .await
    .unwrap();
    env.call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    env.paused("img", "https://news.test/photo.jpg", "Image", "MAIN");
    env.paused("cdn", "https://cdn.other.test/lib.js", "Script", "MAIN");
    env.paused(
        "css",
        "https://static.news.test/site.css",
        "Stylesheet",
        "MAIN",
    );
    env.wait_call("Fetch.failRequest", has_id("img")).await;
    env.wait_call("Fetch.failRequest", has_id("cdn")).await;
    env.wait_call("Fetch.continueRequest", has_id("css")).await;
    let stats = env.call("adblock.stats", json!({})).await.unwrap();
    assert_eq!(stats["total"]["by_policy"], 2);
    assert_eq!(stats["total"]["blocked_by_type"]["image"], 1);
}

#[tokio::test]
async fn navigation_is_continued_first_then_cosmetics_and_js_follow_the_commit() {
    let env = env();
    env.call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    env.call("simplify.site", json!({ "host": "news.test", "js": false }))
        .await
        .unwrap();
    let before = env.methods().len();
    env.paused("nav", "https://news.test/page2", "Document", "MAIN");
    env.wait_call("Fetch.continueRequest", has_id("nav")).await;
    // While the navigation is paused only Fetch.* may be called (renderer commands would
    // wait for the commit, which waits for us).
    let during: Vec<String> = env.methods()[before..].to_vec();
    assert_eq!(during, ["Fetch.continueRequest"]);

    // The commit: script for new documents replaced, current page injected, JS off.
    env.fake.emit_cdp_event(
        TabId(1),
        "Page.frameNavigated",
        json!({ "frame": { "id": "MAIN", "url": "https://news.test/page2" } }),
    );
    env.wait_call("Emulation.setScriptExecutionDisabled", |_| true)
        .await;
    let methods = env.methods()[before..].to_vec();
    let removed = index_of(&methods, "Page.removeScriptToEvaluateOnNewDocument");
    let added = index_of(&methods, "Page.addScriptToEvaluateOnNewDocument");
    assert!(removed < added, "{methods:?}");
    let js = &env
        .fake
        .cdp_calls()
        .into_iter()
        .find(|(m, _)| m == "Emulation.setScriptExecutionDisabled")
        .unwrap()
        .1;
    assert_eq!(js["value"], true);

    // Subresources of the new page are judged against it: script loads are blocked (js off).
    env.paused("s", "https://news.test/app.js", "Script", "MAIN");
    let failed = env.wait_call("Fetch.failRequest", has_id("s")).await;
    assert_eq!(failed["errorReason"], "BlockedByClient");
}

#[tokio::test]
async fn failed_block_answer_falls_back_to_continue() {
    let fail = Arc::new(AtomicBool::new(true));
    let env = env_with(fail);
    env.call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    env.paused("r1", "https://ads.example/x.js", "Script", "MAIN");
    env.wait_call("Fetch.continueRequest", has_id("r1")).await;
}

#[tokio::test]
async fn lost_events_never_switch_fetch_off() {
    let env = env();
    env.call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    // The test runtime is single-threaded: the loops cannot run while we emit, so the
    // 1024-event channels overflow.
    for i in 0..1100 {
        env.paused(&format!("r{i}"), "https://news.test/a.js", "Script", "MAIN");
    }
    env.wait_call("Fetch.continueRequest", has_id("r1099"))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    // No Fetch.disable/enable reset any more: it would also release requests that other
    // clients (net breakpoints) hold on purpose. The coordinator recovers lost pauses itself.
    let methods = env.methods();
    assert!(!methods.iter().any(|m| m == "Fetch.disable"), "{methods:?}");
    assert_eq!(methods.iter().filter(|m| *m == "Fetch.enable").count(), 1);
    for i in [1000, 1050, 1099] {
        assert_eq!(env.answers_for(&format!("r{i}")).len(), 1, "r{i}");
    }
    let stats = env
        .call("adblock.stats", json!({ "tab": 1 }))
        .await
        .unwrap();
    assert!(stats["tabs"][0]["interception"]["lagged"].as_u64().unwrap() > 0);
}

/// Review items: net and adblock on one tab used to overwrite each other's `Fetch.enable`
/// patterns, disable each other's interception, and both answer the same paused request
/// (net's capture listener continued requests adblock had decided to block).
#[tokio::test]
async fn adblock_and_net_share_one_fetch_coordinator() {
    let env = env_with_net();
    env.call("net.capture", json!({ "on": true }))
        .await
        .unwrap();
    env.call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    env.call(
        "net.rule_add",
        json!({ "match": { "url_glob": "*/api/*" }, "action": { "mock": { "body": "{}" } } }),
    )
    .await
    .unwrap();
    env.call(
        "net.breakpoint",
        json!({ "match": { "url_glob": "*/slow/*", "stage": "response" } }),
    )
    .await
    .unwrap();
    // One Fetch.enable per change, each with everybody's patterns (adblock's first).
    let any = json!({ "urlPattern": "*", "requestStage": "Request" });
    let api = json!({ "urlPattern": "*/api/*", "requestStage": "Request" });
    let slow = json!({ "urlPattern": "*/slow/*", "requestStage": "Response" });
    assert_eq!(
        env.fetch_enables(),
        vec![json!([any]), json!([any, api]), json!([any, api, slow])]
    );

    env.paused("ad", "https://ads.example/banner.js", "Script", "MAIN");
    env.paused("ad-api", "https://ads.example/api/track", "XHR", "MAIN");
    env.paused("api", "https://news.test/api/user", "XHR", "MAIN");
    env.paused("css", "https://news.test/site.css", "Stylesheet", "MAIN");
    env.wait_call("Fetch.continueRequest", has_id("css")).await;
    env.wait_call("Fetch.fulfillRequest", has_id("api")).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Exactly one answer each; adblock decides first, so net never mocks or continues a
    // request adblock blocked.
    assert_eq!(env.answers_for("ad"), ["Fetch.failRequest"]);
    assert_eq!(env.answers_for("ad-api"), ["Fetch.failRequest"]);
    assert_eq!(env.answers_for("api"), ["Fetch.fulfillRequest"]);
    assert_eq!(env.answers_for("css"), ["Fetch.continueRequest"]);

    // A response-stage pause for net's breakpoint: adblock no longer continues it.
    env.fake.emit_cdp_event(
        TabId(1),
        "Fetch.requestPaused",
        json!({ "requestId": "slow", "resourceType": "XHR", "responseStatusCode": 200,
                "request": { "url": "https://news.test/slow/x", "method": "GET", "headers": {} } }),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(env.answers_for("slow").is_empty(), "held by the breakpoint");
    let paused = env.call("net.paused_list", json!({})).await.unwrap();
    assert_eq!(paused["count"], 1);
    let pause_id = paused["text"].as_str().unwrap().split(' ').next().unwrap();
    env.call("net.resume", json!({ "pause_id": pause_id }))
        .await
        .unwrap();
    assert_eq!(env.answers_for("slow"), ["Fetch.continueRequest"]);

    // adblock off: net's interception stays (Fetch re-enabled with its patterns only).
    env.call("adblock.apply", json!({ "on": false }))
        .await
        .unwrap();
    assert_eq!(env.fetch_enables().last().unwrap(), &json!([api, slow]));
    assert!(!env.methods().iter().any(|m| m == "Fetch.disable"));
    // The last client leaves: now Fetch is disabled, once.
    for id in ["r1", "b2"] {
        env.call("net.rule_remove", json!({ "id": id }))
            .await
            .unwrap();
    }
    let disables = env
        .methods()
        .iter()
        .filter(|m| *m == "Fetch.disable")
        .count();
    assert_eq!(disables, 1);
}

#[tokio::test]
async fn apply_off_stops_interception_and_cleans_up() {
    let env = env();
    env.call("adblock.apply", json!({ "on": true }))
        .await
        .unwrap();
    let out = env
        .call("adblock.apply", json!({ "on": false }))
        .await
        .unwrap();
    assert_eq!(out["on"], false);
    assert_eq!(out["already"], false);
    let methods = env.methods();
    index_of(&methods, "Fetch.disable");
    index_of(&methods, "Page.removeScriptToEvaluateOnNewDocument");
    // After stopping, paused events are not answered by us any more.
    env.paused("late", "https://ads.example/x.js", "Script", "MAIN");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !env.fake
            .cdp_calls()
            .iter()
            .any(|(m, p)| m == "Fetch.failRequest" && p["requestId"] == "late")
    );
    let again = env
        .call("adblock.apply", json!({ "on": false }))
        .await
        .unwrap();
    assert_eq!(again["already"], true);
}

#[tokio::test]
async fn check_explains_the_decision() {
    let env = env();
    let out = env
        .call(
            "adblock.check",
            json!({ "url": "https://ads.example/a.gif", "source_url": "https://news.test/", "type": "image" }),
        )
        .await
        .unwrap();
    assert_eq!(out["decision"]["action"], "block");
    assert_eq!(out["decision"]["reason"], "filter");
    assert_eq!(out["filter"]["block"], true);
    assert_eq!(out["third_party"], true);
    assert_eq!(out["resource_type"], "image");
    let err = env
        .call(
            "adblock.check",
            json!({ "url": "https://a.test/", "type": "bogus" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    // adblock.check never counts.
    let stats = env.call("adblock.stats", json!({})).await.unwrap();
    assert_eq!(stats["total"]["requests"], 0);
}

#[tokio::test]
async fn toggle_and_policies_persist() {
    let env = env();
    let out = env
        .call(
            "adblock.toggle",
            json!({ "host": "https://www.news.test/x", "on": false }),
        )
        .await
        .unwrap();
    assert_eq!(out["allowlist"], json!(["www.news.test"]));
    env.call(
        "simplify.site",
        json!({ "host": "*.news.test", "fonts": false }),
    )
    .await
    .unwrap();
    let got = env
        .call("simplify.get", json!({ "host": "www.news.test" }))
        .await
        .unwrap();
    assert_eq!(got["matched"], json!(["news.test"]));
    assert_eq!(got["policy"]["fonts"], false);
    assert_eq!(got["adblock"], false);
    assert_eq!(got["content_settings"]["javascript"], "allow");

    // A new service (next start) reads both files back.
    let reloaded = AdblockService::new(env._tmp.path());
    let state = reloaded.gate.state();
    assert!(state.settings.is_allowlisted("www.news.test"));
    assert_eq!(state.policies.sites["news.test"].fonts, Some(false));

    let list = env.call("simplify.list", json!({})).await.unwrap();
    assert_eq!(list["sites"]["news.test"], json!({ "fonts": false }));
    // `clear` forgets the stored fields.
    let cleared = env
        .call(
            "simplify.site",
            json!({ "host": "news.test", "clear": true }),
        )
        .await
        .unwrap();
    assert_eq!(cleared["stored"], json!({}));
    let err = env
        .call("simplify.site", json!({ "host": "bad host" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn update_lists_rejects_unknown_ids() {
    let env = env();
    let err = env
        .call("adblock.update_lists", json!({ "lists": ["nope"] }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert!(err.message.contains("easylist"));
}

#[tokio::test]
async fn engine_loads_from_cached_file() {
    let tmp = tempfile::tempdir().unwrap();
    let service = AdblockService::new(tmp.path());
    // Nothing on disk and no download: no engine, policies only.
    assert!(!service.ensure_engine(false).await.unwrap().loaded);
    std::fs::create_dir_all(service.filters().root.clone()).unwrap();
    std::fs::write(service.filters().list_path("easylist"), RULES).unwrap();
    let status = service.ensure_engine(false).await.unwrap();
    assert!(status.loaded && status.engine_bytes > 0);
    assert_eq!(status.lists, ["easylist"]);
    assert!(service.filters().engine_path().is_file());
    // The next start loads engine.dat instead of compiling.
    let next = AdblockService::new(tmp.path());
    assert!(next.ensure_engine(false).await.unwrap().loaded);
}

#[tokio::test]
async fn reader_runs_readability_in_an_isolated_world() {
    let fake = FakeBackend::with_tabs(&["https://blog.test/post"]);
    let article = json!({
        "title": "A post",
        "byline": "Ann Author",
        "siteName": "Blog",
        "excerpt": "Short",
        "lang": "en",
        "text": "word ".repeat(100).trim().to_string(),
        "applied": false,
    });
    fake.on_cdp(move |_tab, method, params| {
        let reply = match method {
            "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": "F" } } }),
            "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
            "Runtime.evaluate" if params["contextId"] == 9 => {
                json!({ "result": { "type": "object", "value": article.clone() } })
            }
            _ => return None,
        };
        Some(Ok(reply))
    });
    let mut registry = Registry::new();
    crate::register(&mut registry);
    let out = registry
        .call(
            ctx(Caller::Rest, fake.clone()),
            "simplify.reader",
            json!({ "max_chars": 100 }),
        )
        .await
        .unwrap();
    assert_eq!(out["title"], "A post");
    assert_eq!(out["byline"], "Ann Author");
    assert_eq!(out["length"], 499);
    assert_eq!(out["truncated"], true);
    assert_eq!(out["text"].as_str().unwrap().chars().count(), 101);
    let calls = fake.cdp_calls();
    let (_, world) = calls
        .iter()
        .find(|(m, _)| m == "Page.createIsolatedWorld")
        .unwrap();
    assert_eq!(world["frameId"], "F");
    let (_, eval) = calls.iter().find(|(m, _)| m == "Runtime.evaluate").unwrap();
    let expr = eval["expression"].as_str().unwrap();
    assert!(expr.contains("function Readability(doc, options)") && expr.ends_with("(false)"));

    // No article → NOT_FOUND.
    fake.on_cdp(|_tab, method, _| match method {
        "Page.getFrameTree" => Some(Ok(json!({ "frameTree": { "frame": { "id": "F" } } }))),
        "Page.createIsolatedWorld" => Some(Ok(json!({ "executionContextId": 9 }))),
        "Runtime.evaluate" => Some(Ok(json!({ "result": { "type": "object", "value": null } }))),
        _ => None,
    });
    let err = registry
        .call(
            ctx(Caller::Rest, fake.clone()),
            "simplify.reader",
            json!({}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn commands_are_registered_in_their_toolsets() {
    let mut registry = Registry::new();
    crate::register(&mut registry);
    let toolset = |name: &str| registry.get(name).unwrap().toolset;
    for name in [
        "adblock.update_lists",
        "adblock.toggle",
        "adblock.stats",
        "adblock.check",
        "adblock.apply",
        "simplify.site",
        "simplify.get",
        "simplify.list",
        "simplify.measure",
    ] {
        assert_eq!(toolset(name), Toolset::Data, "{name}");
    }
    assert_eq!(toolset("simplify.reader"), Toolset::Core);
    assert!(registry.get("adblock.apply").unwrap().mutates_tab);
}
