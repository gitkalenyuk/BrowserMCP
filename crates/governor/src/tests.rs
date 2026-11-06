//! Governor against `FakeBackend`: the CDP sequences of freeze/discard/thaw, the guards,
//! the event reactions and the automatic policy on a paused tokio clock.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::events::topics as core_topics;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{
    BrowserBackend, Caller, CmdError, Ctx, DataDir, ErrorCode, Event, EventBus, Registry, TabId,
    TabInfo,
};
use serde_json::{Value, json};

use crate::actions::{SET_LIFECYCLE, is_placeholder};
use crate::memory::{MemInfo, MemorySource, ProcInfo};
use crate::service::{FreezeMode, Governor, TabState};
use crate::topics;

const GIB: u64 = 1 << 30;
const A: &str = "https://a.test/";
const B: &str = "https://b.test/";
const C: &str = "https://c.test/";

/// A machine whose RAM the test sets.
struct FakeMemory(Mutex<MemInfo>);

impl FakeMemory {
    fn set(&self, total: u64, available: u64) {
        *self.0.lock().unwrap() = MemInfo { total, available };
    }
}

impl MemorySource for FakeMemory {
    fn read(&self) -> MemInfo {
        *self.0.lock().unwrap()
    }
    fn processes(&self) -> Vec<ProcInfo> {
        vec![ProcInfo {
            pid: 1,
            kind: "renderer".into(),
            mb: 100.0,
        }]
    }
}

/// Three tabs (1 active), the governor commands, one bus, 16 GB RAM with 8 GB free.
struct Harness {
    fake: Arc<FakeBackend>,
    registry: Registry,
    events: EventBus,
    gov: Arc<Governor>,
    mem: Arc<FakeMemory>,
    /// Tabs whose page "plays audio".
    audio: Arc<Mutex<HashSet<i32>>>,
    data: tempfile::TempDir,
}

impl Harness {
    fn new() -> Harness {
        let fake = FakeBackend::with_tabs(&[A, B, C]);
        let audio: Arc<Mutex<HashSet<i32>>> = Arc::default();
        let playing = audio.clone();
        fake.on_cdp(move |tab, method, params| {
            let answer = match method {
                SET_LIFECYCLE => json!({}),
                "Runtime.getHeapUsage" => json!({ "usedSize": 10_485_760.0, "totalSize": 2e7 }),
                "Runtime.evaluate" => {
                    let expr = params["expression"].as_str().unwrap_or_default();
                    let value = if expr.contains("querySelectorAll('audio") {
                        json!(playing.lock().unwrap().contains(&tab.0))
                    } else if expr.contains("scrollX") {
                        json!({ "x": 0, "y": 120 })
                    } else {
                        json!(true)
                    };
                    json!({ "result": { "type": "object", "value": value } })
                }
                _ => return None,
            };
            Some(Ok(answer))
        });
        let mem = Arc::new(FakeMemory(Mutex::new(MemInfo {
            total: 16 * GIB,
            available: 8 * GIB,
        })));
        let data = tempfile::tempdir().unwrap();
        let mut registry = Registry::new();
        crate::register(&mut registry);
        let path = DataDir::from_root(data.path())
            .config()
            .join(crate::config::CONFIG_FILE);
        let gov = registry
            .extensions()
            .insert(Governor::new(Some(path), mem.clone()));
        Harness {
            fake,
            registry,
            events: EventBus::new(),
            gov,
            mem,
            audio,
            data,
        }
    }

    fn backend(&self) -> Arc<dyn BrowserBackend> {
        self.fake.clone()
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(Caller::Rest, self.backend(), self.events.clone());
        self.registry.call(ctx, name, args).await
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    }

    async fn freeze(&self, tab: i32, mode: &str) -> Result<Value, CmdError> {
        self.call("tabs.freeze", json!({ "tab": tab, "mode": mode }))
            .await
    }

    /// CDP calls as `method` or `method state` (lifecycle) for compact asserts.
    fn cdp_log(&self) -> Vec<String> {
        self.fake
            .cdp_calls()
            .into_iter()
            .map(|(m, p)| match p["state"].as_str() {
                Some(state) => format!("{m} {state}"),
                None => m,
            })
            .collect()
    }

    fn url(&self, tab: i32) -> String {
        self.fake
            .tabs()
            .into_iter()
            .find(|t| t.id == TabId(tab))
            .unwrap()
            .url
    }

    /// Feed a bus event straight into the governor (as the service would).
    fn feed(&self, seq: u64, topic: &str, data: Value) {
        let ev = Event {
            seq,
            topic: topic.into(),
            ts_ms: 0,
            data,
        };
        self.gov.on_event(&self.backend(), &self.events, &ev);
    }
}

/// Topics received so far on `rx`.
fn drain(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push((ev.topic, ev.data));
    }
    out
}

// ------------------------------------------------------------------ CDP sequences

#[tokio::test]
async fn freeze_and_thaw_use_the_lifecycle_api() {
    let h = Harness::new();
    let mut rx = h.events.subscribe();
    let out = h.freeze(2, "freeze").await.unwrap();
    assert_eq!(out["state"], "frozen");
    assert_eq!(out["changed"], true);
    // Audio check first (the page is live), then the freeze.
    assert_eq!(
        h.cdp_log(),
        ["Runtime.evaluate", "Page.setWebLifecycleState frozen"]
    );
    // Freezing again changes nothing and sends nothing.
    let again = h.freeze(2, "freeze").await.unwrap();
    assert_eq!(again["changed"], false);
    assert_eq!(h.cdp_log().len(), 2);

    let out = h.freeze(2, "thaw").await.unwrap();
    assert_eq!(out["state"], "live");
    assert_eq!(
        h.cdp_log().last().unwrap(),
        "Page.setWebLifecycleState active"
    );
    let events = drain(&mut rx);
    let topics: Vec<&str> = events
        .iter()
        .map(|(t, _)| t.as_str())
        .filter(|t| t.starts_with("tab."))
        .collect();
    assert_eq!(topics, [topics::TAB_FROZEN, topics::TAB_THAWED]);
    let frozen = &events
        .iter()
        .find(|(t, _)| t == topics::TAB_FROZEN)
        .unwrap()
        .1;
    assert_eq!(frozen, &json!({ "id": 2, "reason": "manual" }));
}

#[tokio::test]
async fn discard_saves_url_title_scroll_and_restore_goes_back() {
    let h = Harness::new();
    h.fake.set_tab(TabId(2), "Page B", B, false);
    let mut rx = h.events.subscribe();

    let out = h.freeze(2, "discard").await.unwrap();
    assert_eq!(out["state"], "discarded");
    assert_eq!(
        out["saved"],
        json!({ "url": B, "title": "Page B", "scroll_x": 0.0, "scroll_y": 120.0 })
    );
    assert!(is_placeholder(&h.url(2)), "{}", h.url(2));
    assert!(
        h.fake
            .calls()
            .iter()
            .any(|c| c.starts_with("navigate 2 data:text/html"))
    );
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Discarded);

    let out = h.freeze(2, "thaw").await.unwrap();
    assert_eq!(out["state"], "live");
    assert_eq!(h.url(2), B);
    assert!(h.fake.calls().contains(&format!("navigate 2 {B}")));
    // The last evaluate scrolls back.
    let (method, params) = h.fake.cdp_calls().last().unwrap().clone();
    assert_eq!(method, "Runtime.evaluate");
    assert!(
        params["expression"]
            .as_str()
            .unwrap()
            .contains("scrollTo(0, 120)")
    );
    let events = drain(&mut rx);
    let discarded = &events
        .iter()
        .find(|(t, _)| t == topics::TAB_DISCARDED)
        .unwrap()
        .1;
    assert_eq!(
        discarded,
        &json!({ "id": 2, "url": B, "title": "Page B", "reason": "manual" })
    );
    assert!(events.iter().any(|(t, d)| t == topics::TAB_RESTORED
        && d == &json!({ "id": 2, "url": B, "reason": "manual" })));
}

#[tokio::test]
async fn discarding_a_frozen_tab_resumes_it_first() {
    let h = Harness::new();
    h.freeze(3, "freeze").await.unwrap();
    h.freeze(3, "discard").await.unwrap();
    assert_eq!(
        h.cdp_log(),
        [
            "Runtime.evaluate",                 // audio check before freezing
            "Page.setWebLifecycleState frozen", // freeze
            "Page.setWebLifecycleState active", // discard: resume first,
            "Runtime.evaluate",                 // unload handler + read the scroll position
        ]
    );
    let (_, params) = &h.fake.cdp_calls()[3];
    assert!(params["expression"].as_str().unwrap().contains("'unload'"));
    // Thaw of a discarded tab is a restore (navigate + scroll), not a lifecycle call.
    h.freeze(3, "thaw").await.unwrap();
    assert_eq!(h.cdp_log()[4..], ["Runtime.evaluate"]);
}

#[tokio::test]
async fn only_web_pages_are_discarded() {
    let h = Harness::new();
    h.fake
        .set_tab(TabId(2), "New tab", bmcp_core::NEW_TAB_URL, false);
    let err = h.freeze(2, "discard").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    // … but they can be frozen.
    assert_eq!(h.freeze(2, "freeze").await.unwrap()["state"], "frozen");
    let err = h.freeze(9, "freeze").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::TabNotFound);
}

// ------------------------------------------------------------------ guards

#[tokio::test]
async fn guards_protect_active_audio_recorded_busy_and_custom_tabs() {
    let h = Harness::new();
    let refused = |r: Result<Value, CmdError>| {
        let err = r.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotInteractable);
        err.message
    };
    // Tab 1 is the active tab.
    assert!(refused(h.freeze(1, "freeze").await).contains("active tab"));
    // Tab 2 plays audio.
    h.audio.lock().unwrap().insert(2);
    assert!(refused(h.freeze(2, "discard").await).contains("playing audio"));
    h.audio.lock().unwrap().clear();
    // Tab 3 is being recorded until the recording stops.
    h.feed(1, "record.started", json!({ "id": "rec-1", "tab": 3 }));
    assert!(refused(h.freeze(3, "freeze").await).contains("being recorded"));
    h.feed(2, "record.stopped", json!({ "id": "rec-1" }));
    assert_eq!(h.freeze(3, "freeze").await.unwrap()["state"], "frozen");
    h.freeze(3, "thaw").await.unwrap();
    // A custom guard from another crate.
    h.gov.add_guard(Arc::new(|tab: &TabInfo| {
        (tab.id == TabId(2)).then(|| "used by job run-7".to_string())
    }));
    assert!(refused(h.freeze(2, "freeze").await).contains("run-7"));
    // Thaw is never refused.
    assert_eq!(h.freeze(2, "thaw").await.unwrap()["changed"], false);
}

#[tokio::test(start_paused = true)]
async fn a_recent_command_makes_a_tab_busy_and_wakes_it() {
    let h = Harness::new();
    h.freeze(2, "freeze").await.unwrap();
    // An agent clicks in tab 2: it is thawed and busy for `busy_secs` (60 s).
    h.feed(
        5,
        core_topics::COMMAND_STARTED,
        json!({ "name": "page.act", "tab": 2, "caller": {"kind": "rest"} }),
    );
    tokio::time::sleep(Duration::from_millis(10)).await; // let the spawned thaw run
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Live);
    let err = h.freeze(2, "freeze").await.unwrap_err();
    assert!(err.message.contains("in use"), "{}", err.message);
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(h.freeze(2, "freeze").await.unwrap()["state"], "frozen");
    // Our own command does not count as use.
    h.feed(
        6,
        core_topics::COMMAND_STARTED,
        json!({ "name": "tabs.freeze", "tab": 2 }),
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Frozen);
}

// ------------------------------------------------------------------ event reactions

#[tokio::test(start_paused = true)]
async fn activating_a_discarded_tab_restores_it() {
    let h = Harness::new();
    h.freeze(2, "discard").await.unwrap();
    h.feed(50, core_topics::TAB_ACTIVATED, json!({ "id": 2 }));
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Live);
    assert_eq!(h.url(2), B);
    let last = h.gov.decisions().pop().unwrap();
    assert_eq!(
        (last.action, last.reason.as_str()),
        (FreezeMode::Thaw, "activated")
    );
}

#[tokio::test(start_paused = true)]
async fn navigation_commands_on_a_discarded_tab_do_not_restore_it() {
    let h = Harness::new();
    h.freeze(2, "discard").await.unwrap();
    let navigations = || {
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("navigate 2"))
            .count()
    };
    let before = navigations();
    // nav.goto loads its own URL: restoring the old one at the same time would race with it.
    h.feed(
        200,
        core_topics::COMMAND_STARTED,
        json!({ "name": "nav.goto", "tab": 2 }),
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(navigations(), before);
    // Any other command restores the page first.
    h.feed(
        201,
        core_topics::COMMAND_STARTED,
        // Read-only commands carry the tab only in their arguments.
        json!({ "name": "page.snapshot", "tab": null, "args": { "tab": 2 } }),
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(navigations(), before + 1);
    assert_eq!(h.url(2), B);
}

#[tokio::test]
async fn user_navigation_away_from_the_placeholder_counts_as_restored() {
    let h = Harness::new();
    let mut rx = h.events.subscribe();
    h.freeze(2, "discard").await.unwrap();
    let discard_seq = std::iter::from_fn(|| rx.try_recv().ok())
        .find(|ev| ev.topic == topics::TAB_DISCARDED)
        .unwrap()
        .seq;
    // A stale update from before the discard: ignored.
    h.feed(
        discard_seq - 1,
        core_topics::TAB_UPDATED,
        json!({ "id": 2, "url": B }),
    );
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Discarded);
    // Updates showing the placeholder: ignored.
    let placeholder = h.url(2);
    h.feed(
        discard_seq + 1,
        core_topics::TAB_UPDATED,
        json!({ "id": 2, "url": placeholder }),
    );
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Discarded);
    // The user clicked "click to restore".
    h.feed(
        discard_seq + 2,
        core_topics::TAB_UPDATED,
        json!({ "id": 2, "url": B }),
    );
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Live);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|(t, d)| t == topics::TAB_RESTORED && d["reason"] == "navigated")
    );
    // Closed tabs are forgotten.
    h.freeze(3, "freeze").await.unwrap();
    h.feed(discard_seq + 3, core_topics::TAB_CLOSED, json!({ "id": 3 }));
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Live);
}

// ------------------------------------------------------------------ automatic policy

#[tokio::test(start_paused = true)]
async fn policy_freezes_then_discards_background_tabs() {
    let h = Harness::new(); // 16 GB → normal: freeze 5 min, discard 30 min
    let backend = h.backend();
    h.gov.tick(&*backend, &h.events).await; // starts the idle clocks
    tokio::time::advance(Duration::from_secs(4 * 60)).await;
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Live);

    tokio::time::advance(Duration::from_secs(61)).await;
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(1)), TabState::Live); // active
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Frozen);
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Frozen);

    tokio::time::advance(Duration::from_secs(25 * 60)).await;
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(1)), TabState::Live);
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Discarded);
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Discarded);
    let reasons: Vec<String> = h.gov.decisions().iter().map(|d| d.reason.clone()).collect();
    assert_eq!(
        reasons,
        [
            "background 5m",
            "background 5m",
            "background 30m",
            "background 30m"
        ]
    );

    // Switching to tab 3 restores it; tab 1 starts idling from now.
    h.fake.activate_tab(TabId(3)).await.unwrap();
    h.feed(100, core_topics::TAB_ACTIVATED, json!({ "id": 3 }));
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Live);
    tokio::time::advance(Duration::from_secs(5 * 60)).await;
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(1)), TabState::Frozen);
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Live);
}

#[tokio::test(start_paused = true)]
async fn low_ram_profile_and_memory_pressure() {
    let h = Harness::new();
    let backend = h.backend();
    // 8 GB machine → low profile, max 8 live tabs, freeze after 2 min.
    h.mem.set(8 * GIB, 4 * GIB);
    assert_eq!(h.gov.limits().profile, crate::Profile::Low);
    h.gov.tick(&*backend, &h.events).await;
    tokio::time::advance(Duration::from_secs(90)).await;
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Live);
    // Memory pressure (5 % free < 20 %): one tab per tick, idle ≥ 60 s.
    h.mem.set(8 * GIB, 8 * GIB / 20);
    h.gov.tick(&*backend, &h.events).await;
    let discarded = [2, 3]
        .iter()
        .filter(|&&t| h.gov.state_of(TabId(t)) == TabState::Discarded)
        .count();
    assert_eq!(discarded, 1);
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Discarded);
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Discarded);
    assert!(
        h.gov.decisions()[0]
            .reason
            .starts_with("memory pressure (5%")
    );
}

#[tokio::test(start_paused = true)]
async fn config_overrides_are_saved_and_disable_the_policy() {
    let h = Harness::new();
    let out = h
        .ok(
            "governor.config",
            json!({ "freeze_after_secs": 10, "max_live_tabs": 2 }),
        )
        .await;
    assert_eq!(out["limits"]["freeze_after_secs"], 10);
    assert_eq!(out["limits"]["profile"], "normal");
    assert_eq!(
        out["overrides"],
        json!({ "freeze_after_secs": 10, "max_live_tabs": 2 })
    );
    let path = h.data.path().join("config").join("governor.json");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"max_live_tabs\": 2")
    );

    // max 2 live tabs: one of the two background tabs (equally idle: the first) is
    // discarded at once.
    let backend = h.backend();
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Discarded);
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Live);
    assert_eq!(h.gov.decisions()[0].reason, "more than 2 live tabs");

    h.ok("governor.config", json!({ "enabled": false })).await;
    tokio::time::advance(Duration::from_secs(3600)).await;
    h.gov.tick(&*backend, &h.events).await;
    assert_eq!(h.gov.state_of(TabId(3)), TabState::Live);

    let out = h.ok("governor.config", json!({ "reset": true })).await;
    assert_eq!(out["overrides"], json!({}));
    assert_eq!(out["limits"]["enabled"], true);
    assert!(
        h.call("governor.config", json!({ "bogus": 1 }))
            .await
            .is_err()
    );
}

// ------------------------------------------------------------------ status, service, parallel

#[tokio::test]
async fn status_reports_memory_tabs_and_decisions() {
    let h = Harness::new();
    h.freeze(2, "freeze").await.unwrap();
    let st = h.ok("governor.status", json!({})).await;
    assert_eq!(st["memory"]["total_mb"], 16 * 1024);
    assert_eq!(st["memory"]["free_pct"], 50.0);
    assert_eq!(st["memory"]["pressure"], false);
    assert_eq!(st["memory"]["renderers"], 1);
    assert_eq!(st["memory"]["browser_mb"], 100.0);
    let tabs = st["tabs"].as_array().unwrap();
    assert_eq!(tabs[0]["protected"], "active tab");
    assert_eq!(tabs[0]["js_heap_mb"], 10.0);
    assert_eq!(tabs[1]["state"], "frozen");
    assert_eq!(tabs[1]["js_heap_mb"], Value::Null);
    assert_eq!(tabs[2]["protected"], Value::Null);
    assert_eq!(st["decisions"][0]["action"], "freeze");
    assert_eq!(st["limits"]["profile"], "normal");
}

#[tokio::test(start_paused = true)]
async fn background_service_reacts_to_bus_events() {
    let h = Harness::new();
    h.freeze(2, "freeze").await.unwrap();
    tokio::spawn(crate::service::run(
        h.gov.clone(),
        h.backend(),
        h.events.clone(),
    ));
    tokio::task::yield_now().await; // let it subscribe
    h.events
        .emit(core_topics::TAB_ACTIVATED, json!({ "id": 2 }));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.gov.state_of(TabId(2)), TabState::Live);
    assert_eq!(
        h.cdp_log().last().unwrap(),
        "Page.setWebLifecycleState active"
    );
}

#[tokio::test]
async fn recommend_parallel_command() {
    let h = Harness::new();
    let out = h
        .ok("governor.recommend_parallel", json!({ "per_slot_mb": 300 }))
        .await;
    assert!(out["parallel"].as_u64().unwrap() >= 1);
    assert!(out["cpus"].as_u64().unwrap() >= 1);
}
