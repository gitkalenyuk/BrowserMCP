//! Unit tests against `FakeBackend` with a scripted page ([`Page`]): it answers the CDP calls
//! the commands make (frame tree, accessibility tree from `tests/fixtures/cdp/`, box model, hit
//! test, the JavaScript probes), so we can check what is sent and how failures are reported.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{BackendError, Caller, CmdError, ErrorCode, Registry, TabId};
use serde_json::{Value, json};

/// Backend ids in `tests/fixtures/cdp/login_form.json`.
const EMAIL: i64 = 15;
const PASSWORD: i64 = 18;
const REMEMBER: i64 = 20;
const SIGN_IN: i64 = 23;
const COOKIES_DIALOG: i64 = 68;
const ACCEPT_ALL: i64 = 71;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../tests/fixtures/cdp/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// The scripted page behind the fake backend.
struct Page {
    tree: Value,
    /// Swapped in for `tree` when a mouse button is pressed (the page "reacts" to a click).
    tree_after_click: Option<Value>,
    loader: String,
    /// Per-node answers of the probe script; default: an enabled, visible button.
    probes: HashMap<i64, Value>,
    /// Nodes that no longer exist (`DOM.resolveNode` fails).
    gone: HashSet<i64>,
    /// Node the hit test reports, whatever the point (e.g. a dialog on top).
    hit_override: Option<i64>,
    history: Value,
}

impl Page {
    fn login() -> Page {
        let mut probes = HashMap::new();
        for id in [EMAIL, PASSWORD] {
            probes.insert(
                id,
                json!({ "connected": true, "tag": "input", "type": "text", "editable": true }),
            );
        }
        probes.insert(
            REMEMBER,
            json!({ "connected": true, "tag": "input", "type": "checkbox", "checked": false }),
        );
        Page {
            tree: fixture("login_form.json"),
            tree_after_click: Some(fixture("login_form_after_submit.json")),
            loader: "loader-1".into(),
            probes,
            gone: HashSet::new(),
            hit_override: None,
            history: json!({ "currentIndex": 0, "entries": [{ "id": 1, "url": "https://acme.com/login" }] }),
        }
    }
}

/// Every node `id` has a 100×8 box at x=100, y=10*id; the hit test inverts that.
fn rect_of(id: i64) -> (f64, f64, f64, f64) {
    (100.0, 10.0 * id as f64, 100.0, 8.0)
}

fn object_id(id: i64) -> String {
    format!("obj-{id}")
}

fn id_of(object: &Value) -> i64 {
    object
        .as_str()
        .and_then(|s| s.strip_prefix("obj-"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn answer(page: &mut Page, method: &str, params: &Value) -> Option<Result<Value, BackendError>> {
    let cdp_err = |m: &str| Err(BackendError::Cdp(format!("CDP error -32000: {m}")));
    let backend = params["backendNodeId"].as_i64().unwrap_or(0);
    let result = match method {
        "Page.getFrameTree" => Ok(json!({ "frameTree": { "frame": { "loaderId": page.loader } } })),
        "Accessibility.getFullAXTree" => Ok(page.tree.clone()),
        "Page.getLayoutMetrics" => Ok(json!({
            "cssLayoutViewport": { "clientWidth": 1280, "clientHeight": 720, "pageX": 0, "pageY": 0 },
            "cssContentSize": { "width": 1280, "height": 1840 },
        })),
        "DOM.resolveNode" if page.gone.contains(&backend) => cdp_err("No node with given id found"),
        "DOM.resolveNode" => Ok(json!({ "object": { "objectId": object_id(backend) } })),
        "DOM.describeNode" => {
            Ok(json!({ "node": { "backendNodeId": id_of(&params["objectId"]) } }))
        }
        "DOM.getContentQuads" => {
            let (x, y, w, h) = rect_of(backend);
            Ok(json!({ "quads": [[x, y, x + w, y, x + w, y + h, x, y + h]] }))
        }
        "DOM.getNodeForLocation" => {
            let y = params["y"].as_i64().unwrap_or(0);
            Ok(json!({ "backendNodeId": page.hit_override.unwrap_or(y / 10) }))
        }
        "Input.dispatchMouseEvent" => {
            if params["type"] == "mousePressed"
                && let Some(next) = page.tree_after_click.take()
            {
                page.tree = next;
            }
            Ok(json!({}))
        }
        "Runtime.callFunctionOn" => {
            let function = params["functionDeclaration"].as_str().unwrap_or_default();
            let this = id_of(&params["objectId"]);
            let value = if function.contains("isConnected") {
                page.probes.get(&this).cloned().unwrap_or_else(
                    || json!({ "connected": true, "tag": "button", "type": "submit" }),
                )
            } else if function.contains("n.host") {
                // Hit check: does the hit node belong to the target?
                let hit = id_of(&params["arguments"][0]["objectId"]);
                json!(hit == this)
            } else if function.contains("[role=\"dialog\"]") {
                // Cover lookup: the Accept button lives in the cookie dialog.
                let cover = if this == ACCEPT_ALL {
                    COOKIES_DIALOG
                } else {
                    this
                };
                return Some(Ok(json!({ "result": { "objectId": object_id(cover) } })));
            } else if function.contains("classList") {
                json!("div#cookie-banner")
            } else if function.contains("this.options") {
                json!({ "error": "no_option", "options": ["Ukraine", "Poland"] })
            } else if function.contains("innerText") {
                json!("Sign in\nWelcome back!")
            } else {
                Value::Null
            };
            Ok(json!({ "result": { "type": "object", "value": value } }))
        }
        "Page.getNavigationHistory" => Ok(page.history.clone()),
        "Page.captureScreenshot" => Ok(json!({ "data": "/9j/AAAA" })),
        "DOM.scrollIntoViewIfNeeded"
        | "DOM.focus"
        | "Input.insertText"
        | "Input.dispatchKeyEvent"
        | "Page.reload"
        | "Page.navigateToHistoryEntry" => Ok(json!({})),
        _ => return None,
    };
    Some(result)
}

struct Harness {
    fake: Arc<FakeBackend>,
    page: Arc<Mutex<Page>>,
    registry: Registry,
}

impl Harness {
    fn new(page: Page) -> Harness {
        let fake = FakeBackend::with_tabs(&["https://acme.com/login"]);
        fake.set_tab(TabId(1), "Sign in – Acme", "https://acme.com/login", false);
        let page = Arc::new(Mutex::new(page));
        let shared = page.clone();
        fake.on_cdp(move |_tab, method, params| {
            answer(&mut shared.lock().unwrap(), method, params)
        });
        let mut registry = Registry::new();
        crate::register(&mut registry);
        Harness {
            fake,
            page,
            registry,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry
            .call(ctx(Caller::Rest, self.fake.clone()), name, args)
            .await
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    /// Take a snapshot and return the ref printed for a backend node.
    async fn snapshot_ref(&self, backend: i64) -> String {
        let out = self.ok("page.snapshot", json!({})).await;
        let text = out["text"].as_str().unwrap().to_string();
        // Look the node up in the snapshot the command just stored (the registry's shared
        // state), instead of guessing ref numbers.
        let ext = self
            .registry
            .extensions()
            .get::<crate::PerceptionState>()
            .unwrap();
        let tab = ext.tab(TabId(1));
        let st = tab.lock().await;
        let (snap, _) = st.history.back().unwrap();
        snap.refs
            .iter()
            .find(|r| r.backend_node_id == backend)
            .map(|r| r.ref_str.clone())
            .unwrap_or_else(|| panic!("no ref for {backend} in:\n{text}"))
    }

    /// CDP calls of one method, as params.
    fn sent(&self, method: &str) -> Vec<Value> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p)
            .collect()
    }

    fn forget_calls(&self) -> usize {
        self.fake.cdp_calls().len()
    }
}

#[tokio::test(start_paused = true)]
async fn snapshot_shows_header_and_refs() {
    let h = Harness::new(Page::login());
    let out = h.ok("page_snapshot", json!({})).await;
    let text = out["text"].as_str().unwrap();
    let first = text.lines().next().unwrap();
    assert_eq!(
        first,
        "snap 1 tab=1 \"Sign in – Acme\" https://acme.com/login vp=1280x720 y=0/1840"
    );
    assert!(text.contains("textbox \"Email\" @1:e"), "{text}");
    assert!(text.contains("dialog \"Cookies\""), "{text}");
    assert_eq!(out["snap"], 1);
}

#[tokio::test(start_paused = true)]
async fn click_sends_mouse_sequence_and_returns_diff() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let out = h
        .ok("page.act", json!({ "ref": sign_in, "action": "click" }))
        .await;

    let mouse = h.sent("Input.dispatchMouseEvent");
    let kinds: Vec<&str> = mouse.iter().map(|p| p["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
    // Centre of the button's box (x 100..200, y 230..238).
    assert_eq!(
        (mouse[1]["x"].as_f64(), mouse[1]["y"].as_f64()),
        (Some(150.0), Some(234.0))
    );
    assert_eq!(mouse[1]["clickCount"], 1);
    assert_eq!(mouse[1]["button"], "left");
    // Hit test at that point.
    assert_eq!(h.sent("DOM.getNodeForLocation")[0]["y"], 234);

    let text = out["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!("clicked {sign_in} button \"Sign in\"")),
        "{text}"
    );
    assert!(text.contains("Wrong password"), "{text}");
    assert!(text.contains("- dialog \"Cookies\""), "{text}");
    assert_eq!(out["navigated"], false);
    // Stealth: never Runtime.enable.
    assert!(h.sent("Runtime.enable").is_empty());
}

#[tokio::test(start_paused = true)]
async fn dblclick_counts_two_clicks() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    h.ok(
        "page.act",
        json!({ "ref": sign_in, "action": "dblclick", "returns": "none" }),
    )
    .await;
    let counts: Vec<i64> = h
        .sent("Input.dispatchMouseEvent")
        .iter()
        .filter(|p| p["type"] != "mouseMoved")
        .map(|p| p["clickCount"].as_i64().unwrap())
        .collect();
    assert_eq!(counts, [1, 1, 2, 2]);
}

#[tokio::test(start_paused = true)]
async fn fill_focuses_selects_inserts_and_fires_change() {
    let h = Harness::new(Page::login());
    let email = h.snapshot_ref(EMAIL).await;
    let before = h.forget_calls();
    let out = h
        .ok(
            "page.act",
            json!({ "ref": email, "action": "fill", "value": "a@b.test", "returns": "none" }),
        )
        .await;
    assert_eq!(out["text"], format!("filled {email} textbox \"Email\""));

    let calls: Vec<(String, Value)> = h.fake.cdp_calls().into_iter().skip(before).collect();
    let focus = calls.iter().position(|(m, _)| m == "DOM.focus").unwrap();
    let insert = calls
        .iter()
        .position(|(m, _)| m == "Input.insertText")
        .unwrap();
    assert_eq!(calls[focus].1["backendNodeId"], EMAIL);
    assert_eq!(calls[insert].1["text"], "a@b.test");
    // Between focus and typing: select-all; after typing: change event.
    let script = |(m, p): &(String, Value)| {
        (m == "Runtime.callFunctionOn")
            .then(|| p["functionDeclaration"].as_str().unwrap().to_string())
    };
    let between: Vec<String> = calls[focus..insert].iter().filter_map(script).collect();
    assert!(
        between.iter().any(|f| f.contains("this.select()")),
        "{between:?}"
    );
    let after: Vec<String> = calls[insert..].iter().filter_map(script).collect();
    assert!(after.iter().any(|f| f.contains("'change'")), "{after:?}");
    // Fill never clicks.
    assert!(h.sent("Input.dispatchMouseEvent").is_empty());
}

#[tokio::test(start_paused = true)]
async fn fill_on_a_button_is_not_interactable() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let err = h
        .call(
            "page.act",
            json!({ "ref": sign_in, "action": "fill", "value": "x" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(err.message.contains("not a text field"), "{}", err.message);
}

#[tokio::test(start_paused = true)]
async fn ref_from_old_document_is_stale() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    h.page.lock().unwrap().loader = "loader-2".into(); // the tab navigated
    let err = h
        .call("page.act", json!({ "ref": sign_in, "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleRef);
    assert_eq!(err.retry, bmcp_core::Retry::AfterSnapshot);
    assert!(h.sent("Input.dispatchMouseEvent").is_empty());

    // A new snapshot gives fresh, working refs (with new numbers).
    let fresh = h.snapshot_ref(SIGN_IN).await;
    assert_ne!(fresh, sign_in);
    h.ok(
        "page.act",
        json!({ "ref": fresh, "action": "click", "returns": "none" }),
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn removed_node_is_stale_and_unknown_ref_not_found() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    h.page.lock().unwrap().gone.insert(SIGN_IN);
    let err = h
        .call("page.act", json!({ "ref": sign_in, "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleRef);

    let err = h
        .call("page.act", json!({ "ref": "e999", "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    let err = h
        .call("page.act", json!({ "ref": "button", "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test(start_paused = true)]
async fn covered_button_names_the_dialog_and_its_buttons() {
    let mut page = Page::login();
    page.hit_override = Some(ACCEPT_ALL); // the cookie dialog lies over everything
    let h = Harness::new(page);
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let err = h
        .call("page.act", json!({ "ref": sign_in, "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(
        err.message
            .contains("is covered by dialog \"Cookies\" (div#cookie-banner)"),
        "{}",
        err.message
    );
    let hint = err.hint.unwrap();
    assert!(hint.contains("button \"Accept all\" @"), "{hint}");
    assert!(hint.contains("button \"Reject all\" @"), "{hint}");
    // Nothing was clicked.
    assert!(h.sent("Input.dispatchMouseEvent").is_empty());

    // Clicking the Accept button itself works (the hit test finds it).
    let accept = h.snapshot_ref(ACCEPT_ALL).await;
    h.ok(
        "page.act",
        json!({ "ref": accept, "action": "click", "returns": "none" }),
    )
    .await;
    assert_eq!(h.sent("Input.dispatchMouseEvent").len(), 3);
}

#[tokio::test(start_paused = true)]
async fn disabled_and_hidden_are_not_interactable() {
    let mut page = Page::login();
    page.probes.insert(
        SIGN_IN,
        json!({ "connected": true, "tag": "button", "disabled": true }),
    );
    page.probes.insert(
        ACCEPT_ALL,
        json!({ "connected": true, "tag": "button", "hidden": true }),
    );
    let h = Harness::new(page);
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let err = h
        .call("page.act", json!({ "ref": sign_in, "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(err.message.ends_with("is disabled"), "{}", err.message);
    assert_eq!(err.retry, bmcp_core::Retry::AfterWait);

    let accept = h.snapshot_ref(ACCEPT_ALL).await;
    let err = h
        .call("page.act", json!({ "ref": accept, "action": "click" }))
        .await
        .unwrap_err();
    assert!(err.message.contains("hidden"), "{}", err.message);
}

#[tokio::test(start_paused = true)]
async fn hidden_tab_refuses_input_instead_of_dropping_it() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    // The fake answers `Runtime.evaluate` (here: document.visibilityState) with this text.
    h.fake.set_text(TabId(1), "hidden");
    let err = h
        .call("page.act", json!({ "ref": sign_in, "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(err.message.contains("is hidden"), "{}", err.message);
    assert!(h.sent("Input.dispatchMouseEvent").is_empty());
    // Scrolling does not need input events and still works.
    h.ok(
        "page.act",
        json!({ "ref": sign_in, "action": "scroll", "returns": "none" }),
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn check_clicks_only_when_needed() {
    let h = Harness::new(Page::login());
    let remember = h.snapshot_ref(REMEMBER).await;
    // The fake checkbox never changes state, so a click "does not work".
    let err = h
        .call("page.act", json!({ "ref": remember, "action": "check" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(
        err.message.contains("did not make it checked"),
        "{}",
        err.message
    );
    assert_eq!(h.sent("Input.dispatchMouseEvent").len(), 3);

    // Already unchecked: no click at all.
    let out = h
        .ok(
            "page.act",
            json!({ "ref": remember, "action": "uncheck", "returns": "none" }),
        )
        .await;
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .contains("was already unchecked")
    );
    assert_eq!(h.sent("Input.dispatchMouseEvent").len(), 3);
}

#[tokio::test(start_paused = true)]
async fn press_focuses_then_sends_keys() {
    let h = Harness::new(Page::login());
    let password = h.snapshot_ref(PASSWORD).await;
    h.ok(
        "page.act",
        json!({ "ref": password, "action": "press", "value": "Enter", "returns": "none" }),
    )
    .await;
    assert_eq!(h.sent("DOM.focus")[0]["backendNodeId"], PASSWORD);
    let keys = h.sent("Input.dispatchKeyEvent");
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0]["key"], "Enter");
    assert_eq!(keys[0]["text"], "\r");

    let err = h
        .call(
            "page.act",
            json!({ "ref": password, "action": "press", "value": "Hyper+Q" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(start_paused = true)]
async fn select_unknown_option_lists_candidates() {
    let h = Harness::new(Page::login());
    let email = h.snapshot_ref(EMAIL).await;
    let err = h
        .call(
            "page.act",
            json!({ "ref": email, "action": "select", "value": "Mars" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.candidates, vec![json!("Ukraine"), json!("Poland")]);
}

#[tokio::test(start_paused = true)]
async fn missing_value_and_missing_ref_are_invalid() {
    let h = Harness::new(Page::login());
    let email = h.snapshot_ref(EMAIL).await;
    let err = h
        .call("page.act", json!({ "ref": email, "action": "fill" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = h
        .call("page.act", json!({ "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = h
        .call("page.act", json!({ "ref": email, "action": "explode" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(start_paused = true)]
async fn batch_runs_steps_and_diffs_once() {
    let h = Harness::new(Page::login());
    let email = h.snapshot_ref(EMAIL).await;
    let password = h.snapshot_ref(PASSWORD).await;
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let out = h
        .ok(
            "page.act_batch",
            json!({ "steps": [
                { "ref": email, "action": "fill", "value": "a@b.test" },
                { "ref": password, "action": "fill", "value": "secret" },
                { "ref": sign_in, "action": "click" },
            ]}),
        )
        .await;
    assert_eq!(out["ok"], true);
    assert_eq!(out["steps"].as_array().unwrap().len(), 3);
    assert!(out["text"].as_str().unwrap().contains("Wrong password"));
    let typed: Vec<Value> = h
        .sent("Input.insertText")
        .iter()
        .map(|p| p["text"].clone())
        .collect();
    assert_eq!(typed, [json!("a@b.test"), json!("secret")]);
}

#[tokio::test(start_paused = true)]
async fn batch_stops_on_error() {
    let h = Harness::new(Page::login());
    let email = h.snapshot_ref(EMAIL).await;
    let out = h
        .ok(
            "page.act_batch",
            json!({ "steps": [
                { "ref": email, "action": "fill", "value": "a@b.test" },
                { "ref": "e999", "action": "click" },
                { "ref": email, "action": "fill", "value": "never" },
            ]}),
        )
        .await;
    assert_eq!(out["ok"], false);
    let steps = out["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1]["ok"], false);
    assert_eq!(steps[1]["error"]["code"], "NOT_FOUND");
    assert_eq!(h.sent("Input.insertText").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn find_gives_refs_usable_by_act() {
    let h = Harness::new(Page::login());
    let out = h
        .ok(
            "page.find",
            json!({ "role": "button", "name": "accept all" }),
        )
        .await;
    let text = out["text"].as_str().unwrap();
    assert!(text.starts_with("find 1: 1 of 1 matches"), "{text}");
    let r = text
        .split_whitespace()
        .find(|w| w.starts_with('@'))
        .unwrap();
    h.ok(
        "page.act",
        json!({ "ref": r, "action": "hover", "returns": "none" }),
    )
    .await;
    let mouse = h.sent("Input.dispatchMouseEvent");
    assert_eq!(mouse.len(), 1);
    assert_eq!(mouse[0]["y"].as_f64(), Some(714.0));
}

#[tokio::test(start_paused = true)]
async fn diff_and_read() {
    let h = Harness::new(Page::login());
    let err = h.call("page.diff", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    h.ok("page.snapshot", json!({})).await;
    let out = h.ok("page.diff", json!({})).await;
    assert_eq!(out["text"], "diff 1→2: no changes");

    {
        let mut page = h.page.lock().unwrap();
        page.tree = fixture("login_form_after_submit.json");
    }
    let out = h.ok("page.diff", json!({ "since": 1 })).await;
    let text = out["text"].as_str().unwrap();
    assert!(text.starts_with("diff 1→3:"), "{text}");
    assert!(text.contains("- dialog \"Cookies\""), "{text}");

    let email = h.snapshot_ref(EMAIL).await;
    let out = h
        .ok("page.read", json!({ "ref": email, "max_chars": 7 }))
        .await;
    assert_eq!(out["text"], "Sign in");
    assert_eq!(out["truncated"], true);
}

#[tokio::test(start_paused = true)]
async fn screenshot_clips_to_ref_at_half_scale() {
    let h = Harness::new(Page::login());
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let out = h.ok("page.screenshot", json!({ "ref": sign_in })).await;
    assert_eq!(out["mime_type"], "image/jpeg");
    assert_eq!(out["data"], "/9j/AAAA");
    assert_eq!(
        (out["width"].as_u64(), out["height"].as_u64()),
        (Some(50), Some(4))
    );
    let shot = &h.sent("Page.captureScreenshot")[0];
    assert_eq!(shot["format"], "jpeg");
    assert_eq!(
        shot["clip"],
        json!({ "x": 100.0, "y": 230.0, "width": 100.0, "height": 8.0, "scale": 0.5 })
    );
}

#[tokio::test(start_paused = true)]
async fn history_navigation_and_waits() {
    let h = Harness::new(Page::login());
    let err = h.call("nav.back", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    h.page.lock().unwrap().history = json!({
        "currentIndex": 1,
        "entries": [{ "id": 7, "url": "https://acme.com/" }, { "id": 8, "url": "https://acme.com/login" }],
    });
    h.ok("nav.back", json!({})).await;
    assert_eq!(h.sent("Page.navigateToHistoryEntry")[0]["entryId"], 7);

    h.ok("nav.reload", json!({})).await;
    assert_eq!(h.sent("Page.reload").len(), 1);

    h.ok("nav.wait", json!({ "for": "url", "value": "/login" }))
        .await;
    let err = h
        .call(
            "nav.wait",
            json!({ "for": "url", "value": "/dashboard", "timeout_secs": 2 }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Timeout);
    let err = h
        .call("nav.wait", json!({ "for": "text" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(start_paused = true)]
async fn input_keys_sends_chords() {
    let h = Harness::new(Page::login());
    let out = h
        .ok("input.keys", json!({ "keys": "Control+A Delete" }))
        .await;
    assert_eq!(out["pressed"], 2);
    let keys = h.sent("Input.dispatchKeyEvent");
    assert_eq!(keys.len(), 6); // ctrl down, a down, a up, ctrl up, delete down, delete up
    assert_eq!(keys[1]["commands"], json!(["selectAll"]));
}

#[tokio::test(start_paused = true)]
async fn per_tab_state_survives_between_calls() {
    let h = Harness::new(Page::login());
    h.ok("page.snapshot", json!({})).await;
    let out = h.ok("page.snapshot", json!({})).await;
    // Same refs, next snapshot id.
    assert_eq!(out["snap"], 2);
    assert!(out["text"].as_str().unwrap().contains("@2:e2"));
}

#[test]
fn all_commands_are_registered_in_core() {
    let mut registry = Registry::new();
    crate::register(&mut registry);
    let names: Vec<&str> = registry.list(None).iter().map(|c| c.name).collect();
    for name in [
        "page.snapshot",
        "page.find",
        "page.act",
        "page.act_batch",
        "page.diff",
        "page.read",
        "page.screenshot",
        "nav.back",
        "nav.forward",
        "nav.reload",
        "nav.wait",
        "input.keys",
    ] {
        assert!(names.contains(&name), "{name} missing");
        assert_eq!(
            registry.get(name).unwrap().toolset,
            bmcp_core::Toolset::Core
        );
    }
    // The act schema exposes `ref`, not the Rust field name.
    let schema = &registry.get("page_act").unwrap().args_schema;
    assert!(schema["properties"].get("ref").is_some(), "{schema}");
}
