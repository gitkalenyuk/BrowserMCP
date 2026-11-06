//! Tests against `FakeBackend`: CDP calls out, scripted `Fetch.requestPaused` events in.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::testing::FakeBackend;
use bmcp_core::{BackendError, BoxFuture, BrowserBackend, TabId};
use serde_json::{Value, json};

use crate::fetch::{BY_DEFAULT, BY_DISABLE, BY_FALLBACK, BY_HOLD_LIMIT, HOLD_LIMIT, SWEEP_GRACE};
use crate::{
    Answer, AnswerError, Answered, CdpShare, FetchClient, FetchHandler, PausedRequest, Verdict,
};

const TAB: TabId = TabId(1);

struct Setup {
    fake: Arc<FakeBackend>,
    backend: Arc<dyn BrowserBackend>,
    share: CdpShare,
}

/// One tab whose CDP answers `{}` to domain and Fetch calls; `failing` methods fail.
fn setup_failing(failing: &'static [&'static str]) -> Setup {
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);
    fake.on_cdp(move |_, method, _| {
        if failing.contains(&method) {
            return Some(Err(BackendError::Cdp("Invalid InterceptionId.".into())));
        }
        let known = method.starts_with("Fetch.")
            || method.ends_with(".enable")
            || method.ends_with(".disable");
        known.then(|| Ok(json!({})))
    });
    Setup {
        backend: fake.clone(),
        fake,
        share: CdpShare::default(),
    }
}

fn setup() -> Setup {
    setup_failing(&[])
}

impl Setup {
    /// Params of the CDP calls with this method, in order.
    fn calls(&self, method: &str) -> Vec<Value> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p)
            .collect()
    }

    /// How many answers (`Fetch.*Request`/`continueResponse`) went out for this request id.
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

    fn pause(&self, id: &str, url: &str) {
        self.fake.emit_cdp_event(
            TAB,
            "Fetch.requestPaused",
            json!({ "requestId": id, "resourceType": "Script",
                    "request": { "url": url, "method": "GET", "headers": {} } }),
        );
    }

    async fn client(&self, name: &str, order: u8, patterns: Value, handler: Arc<TestClient>) {
        self.share
            .set_fetch(
                &self.backend,
                TAB,
                FetchClient {
                    name: name.into(),
                    order,
                    patterns: serde_json::from_value(patterns).unwrap(),
                    handler,
                },
            )
            .await
            .unwrap();
    }
}

type Decide = Box<dyn Fn(&PausedRequest) -> Option<Verdict> + Send + Sync>;

/// A client whose verdicts come from a closure (`None` = never decides).
struct TestClient {
    decide: Decide,
    offered: Mutex<Vec<String>>,
    done: Mutex<Vec<(String, String)>>,
}

impl TestClient {
    fn new(
        decide: impl Fn(&PausedRequest) -> Option<Verdict> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(TestClient {
            decide: Box::new(decide),
            offered: Mutex::new(Vec::new()),
            done: Mutex::new(Vec::new()),
        })
    }

    fn offered(&self) -> Vec<String> {
        self.offered.lock().unwrap().clone()
    }

    fn done(&self) -> Vec<(String, String)> {
        self.done.lock().unwrap().clone()
    }
}

impl FetchHandler for TestClient {
    fn on_paused<'a>(&'a self, req: &'a PausedRequest) -> BoxFuture<'a, Verdict> {
        self.offered.lock().unwrap().push(req.request_id.clone());
        match (self.decide)(req) {
            Some(verdict) => Box::pin(async move { verdict }),
            None => Box::pin(std::future::pending()),
        }
    }

    fn done(&self, req: &PausedRequest, answered: &Answered<'_>) {
        self.done
            .lock()
            .unwrap()
            .push((req.request_id.clone(), answered.by.to_string()));
    }
}

/// Let the event loop and the tasks it spawned run.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

fn req(pattern: &str) -> Value {
    json!({ "urlPattern": pattern, "requestStage": "Request" })
}

// ------------------------------------------------------------------ domains

#[tokio::test]
async fn domains_are_enabled_once_and_disabled_after_the_last_holder() {
    let s = setup();
    let net = json!({ "maxPostDataSize": 65536 });
    s.share
        .acquire(&s.backend, TAB, "Network", "record:1", json!({}))
        .await
        .unwrap();
    s.share
        .acquire(&s.backend, TAB, "Network", "net", net.clone())
        .await
        .unwrap();
    // Same holder again: counted once, nothing sent.
    s.share
        .acquire(&s.backend, TAB, "Network", "net", net.clone())
        .await
        .unwrap();
    assert_eq!(s.calls("Network.enable"), vec![json!({}), net.clone()]);
    assert_eq!(s.share.holders(TAB, "Network"), ["net", "record:1"]);

    s.share
        .release(&s.backend, TAB, "Network", "net")
        .await
        .unwrap();
    s.share
        .release(&s.backend, TAB, "Network", "nobody")
        .await
        .unwrap();
    assert!(
        s.calls("Network.disable").is_empty(),
        "record still needs it"
    );
    s.share
        .release(&s.backend, TAB, "Network", "record:1")
        .await
        .unwrap();
    assert_eq!(s.calls("Network.disable").len(), 1);
    assert!(s.share.holders(TAB, "Network").is_empty());

    // Page is enabled once and never disabled (others rely on it).
    for holder in ["a", "b"] {
        s.share
            .acquire(&s.backend, TAB, "Page", holder, json!({}))
            .await
            .unwrap();
    }
    for holder in ["a", "b"] {
        s.share
            .release(&s.backend, TAB, "Page", holder)
            .await
            .unwrap();
    }
    s.share
        .acquire(&s.backend, TAB, "Page", "c", json!({}))
        .await
        .unwrap();
    assert_eq!(s.calls("Page.enable").len(), 1);
    assert!(s.calls("Page.disable").is_empty());
}

#[tokio::test]
async fn a_failed_enable_does_not_count_as_held() {
    let s = setup_failing(&["Network.enable"]);
    assert!(
        s.share
            .acquire(&s.backend, TAB, "Network", "net", json!({}))
            .await
            .is_err()
    );
    assert!(s.share.holders(TAB, "Network").is_empty());
}

// ------------------------------------------------------------------ Fetch.enable

#[tokio::test]
async fn one_fetch_enable_with_the_union_of_all_clients() {
    let s = setup();
    let pass = TestClient::new(|_| Some(Verdict::Pass));
    s.client("net", 20, json!([req("*/api/*"), req("*")]), pass.clone())
        .await;
    s.client("adblock", 10, json!([req("*")]), pass.clone())
        .await;
    assert_eq!(
        s.calls("Fetch.enable"),
        vec![
            json!({ "patterns": [req("*/api/*"), req("*")] }),
            // adblock comes first; the shared "*" is listed once.
            json!({ "patterns": [req("*"), req("*/api/*")] }),
        ]
    );
    // Same patterns again: nothing sent.
    s.client("adblock", 10, json!([req("*")]), pass.clone())
        .await;
    assert_eq!(s.calls("Fetch.enable").len(), 2);

    s.share.clear_fetch(&s.backend, TAB, "net").await.unwrap();
    assert_eq!(
        s.calls("Fetch.enable").last().unwrap(),
        &json!({ "patterns": [req("*")] })
    );
    assert!(
        s.calls("Fetch.disable").is_empty(),
        "adblock is still there"
    );
    s.share
        .clear_fetch(&s.backend, TAB, "adblock")
        .await
        .unwrap();
    assert_eq!(s.calls("Fetch.disable").len(), 1);
    assert!(s.share.fetch_patterns(TAB).is_empty());
}

#[tokio::test]
async fn a_failed_fetch_enable_keeps_the_old_registration() {
    let s = setup_failing(&["Fetch.enable"]);
    let pass = TestClient::new(|_| Some(Verdict::Pass));
    let err = s
        .share
        .set_fetch(
            &s.backend,
            TAB,
            FetchClient {
                name: "net".into(),
                order: 20,
                patterns: vec![req("*")],
                handler: pass,
            },
        )
        .await;
    assert!(err.is_err());
    assert!(s.share.fetch_patterns(TAB).is_empty());
}

// ------------------------------------------------------------------ pauses

#[tokio::test]
async fn clients_are_asked_in_order_and_each_pause_is_answered_once() {
    let s = setup();
    // "adblock" blocks ads and passes everything else.
    let adblock = TestClient::new(|r| {
        Some(if r.url().contains("ads.") {
            Verdict::Answer(Answer::fail("BlockedByClient"))
        } else {
            Verdict::Pass
        })
    });
    // "net" mocks /api/ and passes the rest; its pattern does not cover /static/.
    let net = TestClient::new(|r| {
        Some(if r.url().contains("/api/") {
            Verdict::Answer(Answer::new(
                "Fetch.fulfillRequest",
                json!({ "responseCode": 200, "responseHeaders": [], "body": "" }),
            ))
        } else {
            Verdict::Pass
        })
    });
    s.client("net", 20, json!([req("*://a.test/*")]), net.clone())
        .await;
    s.client("adblock", 10, json!([req("*")]), adblock.clone())
        .await;

    s.pause("1", "https://a.test/ads.js?ads.x");
    s.pause("2", "https://a.test/api/user");
    s.pause("3", "https://a.test/page.css");
    s.pause("4", "https://b.test/static/x.js");
    settle().await;

    assert_eq!(s.answers_for("1"), ["Fetch.failRequest"]);
    assert_eq!(s.answers_for("2"), ["Fetch.fulfillRequest"]);
    assert_eq!(s.answers_for("3"), ["Fetch.continueRequest"]);
    assert_eq!(s.answers_for("4"), ["Fetch.continueRequest"]);
    assert_eq!(adblock.offered(), ["1", "2", "3", "4"]);
    // Blocked first: net never saw request 1; request 4 is outside net's pattern.
    assert_eq!(net.offered(), ["2", "3"]);
    // Every interested client hears who answered.
    let mut done = net.done();
    done.sort();
    assert_eq!(
        done,
        [
            ("1".to_string(), "adblock".to_string()),
            ("2".to_string(), "net".to_string()),
            ("3".to_string(), BY_DEFAULT.to_string()),
        ]
    );
    let stats = s.share.fetch_stats(TAB);
    assert_eq!((stats.paused, stats.answered, stats.pending), (4, 4, 0));
}

#[tokio::test]
async fn held_requests_are_answered_later_exactly_once() {
    let s = setup();
    let net = TestClient::new(|_| Some(Verdict::Hold));
    s.client("net", 20, json!([req("*")]), net.clone()).await;
    s.pause("1", "https://a.test/x");
    settle().await;
    assert!(s.answers_for("1").is_empty(), "held");
    assert_eq!(s.share.fetch_stats(TAB).pending, 1);

    s.share
        .answer(TAB, "1", Answer::fail("Aborted"), "net")
        .await
        .unwrap();
    assert_eq!(
        s.share
            .answer(TAB, "1", Answer::continue_request(), "net")
            .await,
        Err(AnswerError::NotPaused)
    );
    assert_eq!(s.answers_for("1"), ["Fetch.failRequest"]);
    assert_eq!(
        s.calls("Fetch.failRequest"),
        vec![json!({ "requestId": "1", "errorReason": "Aborted" })]
    );
}

#[tokio::test]
async fn a_failed_answer_falls_back_to_continue() {
    let s = setup_failing(&["Fetch.fulfillRequest"]);
    let net = TestClient::new(|_| Some(Verdict::Hold));
    s.client("net", 20, json!([req("*")]), net.clone()).await;
    s.pause("1", "https://a.test/x");
    settle().await;
    let err = s
        .share
        .answer(
            TAB,
            "1",
            Answer::new("Fetch.fulfillRequest", json!({ "responseCode": 200 })),
            "net",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AnswerError::Cdp(_)));
    assert_eq!(
        s.answers_for("1"),
        ["Fetch.fulfillRequest", "Fetch.continueRequest"]
    );
}

#[tokio::test]
async fn a_panicking_handler_still_gets_its_request_continued() {
    let s = setup();
    let bad = TestClient::new(|_| panic!("handler bug"));
    s.client("net", 20, json!([req("*")]), bad.clone()).await;
    s.pause("1", "https://a.test/x");
    settle().await;
    assert_eq!(s.answers_for("1"), ["Fetch.continueRequest"]);
    assert_eq!(bad.done(), [("1".to_string(), BY_FALLBACK.to_string())]);
}

#[tokio::test(start_paused = true)]
async fn slow_handlers_and_forgotten_holds_are_continued() {
    let s = setup();
    let slow = TestClient::new(|r| r.url().ends_with("/hold").then_some(Verdict::Hold));
    s.client("net", 20, json!([req("*")]), slow.clone()).await;
    s.pause("slow", "https://a.test/slow");
    s.pause("held", "https://a.test/hold");
    settle().await;
    assert!(s.answers_for("slow").is_empty());

    tokio::time::sleep(crate::fetch::HANDLER_TIMEOUT + Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(s.answers_for("slow"), ["Fetch.continueRequest"]);
    assert!(s.answers_for("held").is_empty());

    tokio::time::sleep(HOLD_LIMIT).await;
    settle().await;
    assert_eq!(s.answers_for("held"), ["Fetch.continueRequest"]);
    assert!(
        slow.done()
            .contains(&("held".to_string(), BY_HOLD_LIMIT.to_string()))
    );
}

#[tokio::test]
async fn disabling_fetch_tells_holders_their_requests_are_gone() {
    let s = setup();
    let net = TestClient::new(|_| Some(Verdict::Hold));
    s.client("net", 20, json!([req("*")]), net.clone()).await;
    s.pause("1", "https://a.test/x");
    settle().await;
    s.share.clear_fetch(&s.backend, TAB, "net").await.unwrap();
    assert_eq!(net.done(), [("1".to_string(), BY_DISABLE.to_string())]);
    assert_eq!(
        s.share
            .answer(TAB, "1", Answer::continue_request(), "net")
            .await,
        Err(AnswerError::NotPaused)
    );
    // Chromium's Fetch.disable released it; we sent nothing for it.
    assert!(s.answers_for("1").is_empty());
}

// ------------------------------------------------------------------ lost events

#[tokio::test(start_paused = true)]
async fn lost_pauses_are_swept_without_disabling_fetch() {
    let s = setup();
    // Holds requests to /held, passes the rest (they are continued by default).
    let net = TestClient::new(|r| {
        Some(if r.url().ends_with("/held") {
            Verdict::Hold
        } else {
            Verdict::Pass
        })
    });
    s.client("net", 20, json!([req("*")]), net.clone()).await;
    s.pause("interception-job-1.0", "https://a.test/one");
    settle().await;
    assert_eq!(
        s.answers_for("interception-job-1.0"),
        ["Fetch.continueRequest"]
    );

    // The test runtime is single-threaded: the coordinator cannot run while we emit, so its
    // 1024-event channel overflows and drops the oldest events — the pause of job 2.
    s.pause("interception-job-2.0", "https://a.test/two");
    for i in 0..1100 {
        s.fake.emit_cdp_event(
            TAB,
            "Network.dataReceived",
            json!({ "requestId": format!("n{i}") }),
        );
    }
    s.pause("interception-job-3.0", "https://a.test/held");
    settle().await;
    assert!(s.share.fetch_stats(TAB).lagged > 0);
    assert!(s.answers_for("interception-job-2.0").is_empty(), "lost");

    tokio::time::sleep(SWEEP_GRACE + Duration::from_millis(100)).await;
    settle().await;
    // The lost one is released, once; the answered and the held ones are left alone.
    assert_eq!(
        s.answers_for("interception-job-2.0"),
        ["Fetch.continueRequest"]
    );
    assert_eq!(
        s.answers_for("interception-job-1.0"),
        ["Fetch.continueRequest"]
    );
    assert!(s.answers_for("interception-job-3.0").is_empty(), "held");
    assert!(s.calls("Fetch.disable").is_empty());
    assert_eq!(s.calls("Fetch.enable").len(), 1);
    assert!(s.share.fetch_stats(TAB).swept >= 1);
    // The held one can still be answered normally.
    s.share
        .answer(
            TAB,
            "interception-job-3.0",
            Answer::continue_request(),
            "net",
        )
        .await
        .unwrap();
}

// ------------------------------------------------------------------ auto-attach

/// One tab whose CDP answers `{}` to `Target.setAutoAttach` (page and child sessions).
fn attach_setup() -> Setup {
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);
    fake.on_cdp(|_, method, _| (method == "Target.setAutoAttach").then(|| Ok(json!({}))));
    Setup {
        backend: fake.clone(),
        fake,
        share: CdpShare::default(),
    }
}

#[tokio::test]
async fn auto_attach_pauses_when_any_holder_wants_it() {
    let s = attach_setup();
    // OOPIF support first: switched on (existing targets announced), no pausing.
    assert!(
        s.share
            .hold_auto_attach(&s.backend, TAB, None, "frames", false)
            .await
            .unwrap()
    );
    // An identity tab: pausing on top (one more call with the merged setting); it was on
    // already, so the existing targets were not announced again.
    assert!(
        !s.share
            .hold_auto_attach(&s.backend, TAB, None, "identity", true)
            .await
            .unwrap()
    );
    // frames again (a later snapshot): the merged setting is unchanged, nothing is sent, and
    // it can no longer switch the pausing off.
    assert!(
        !s.share
            .hold_auto_attach(&s.backend, TAB, None, "frames", false)
            .await
            .unwrap()
    );
    assert_eq!(
        s.calls("Target.setAutoAttach"),
        [
            json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true}),
            json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
        ]
    );
    assert_eq!(s.share.auto_attach_waits(TAB, None), Some(true));
    assert_eq!(
        s.share.auto_attach_holders(TAB, None),
        ["frames", "identity"]
    );

    // Re-announce (frames after lost events): off, then on with the merged setting.
    s.share
        .reannounce_auto_attach(&s.backend, TAB, None)
        .await
        .unwrap();
    let calls = s.calls("Target.setAutoAttach");
    assert_eq!(calls[2]["autoAttach"], false);
    assert_eq!(
        calls[3],
        json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true})
    );

    // identity leaves: back to no pausing; frames leaves: off.
    s.share
        .release_auto_attach(&s.backend, TAB, None, "identity")
        .await
        .unwrap();
    s.share
        .release_auto_attach(&s.backend, TAB, None, "frames")
        .await
        .unwrap();
    let calls = s.calls("Target.setAutoAttach");
    assert_eq!(calls.len(), 6);
    assert_eq!(
        calls[4],
        json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true})
    );
    assert_eq!(calls[5]["autoAttach"], false);
    assert_eq!(s.share.auto_attach_waits(TAB, None), None);
    // Releasing again is a no-op.
    s.share
        .release_auto_attach(&s.backend, TAB, None, "frames")
        .await
        .unwrap();
    assert_eq!(s.calls("Target.setAutoAttach").len(), 6);
}

#[tokio::test]
async fn auto_attach_is_per_session_and_failures_undo() {
    let s = attach_setup();
    s.share
        .hold_auto_attach(&s.backend, TAB, None, "identity", true)
        .await
        .unwrap();
    // A child session has its own setting.
    s.share
        .hold_auto_attach(&s.backend, TAB, Some("S1".into()), "frames", false)
        .await
        .unwrap();
    let calls = s.calls("Target.setAutoAttach");
    assert_eq!(calls[1]["sessionId"], "S1");
    assert_eq!(calls[1]["waitForDebuggerOnStart"], false);
    assert_eq!(s.share.auto_attach_waits(TAB, None), Some(true));
    assert_eq!(
        s.share.auto_attach_waits(TAB, Some("S1".into())),
        Some(false)
    );
    s.share.forget_session(TAB, "S1");
    assert_eq!(s.share.auto_attach_waits(TAB, Some("S1".into())), None);

    // A failed call leaves no holder behind.
    s.fake.fail_next(BackendError::Cdp("Not allowed".into()));
    assert!(
        s.share
            .hold_auto_attach(&s.backend, TAB, Some("S2".into()), "frames", false)
            .await
            .is_err()
    );
    assert!(
        s.share
            .auto_attach_holders(TAB, Some("S2".into()))
            .is_empty()
    );
    assert_eq!(s.share.auto_attach_waits(TAB, None), Some(true));
}
