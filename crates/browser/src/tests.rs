//! Unit tests against `FakeBackend` with a scripted page ([`Page`]): it answers the CDP calls
//! the commands make (frame tree, accessibility tree from `tests/fixtures/cdp/`, box model, hit
//! test, the JavaScript probes), so we can check what is sent and how failures are reported.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{BackendError, BrowserBackend, Caller, CmdError, ErrorCode, Registry, TabId};
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
    /// A `pointer-events: none` overlay over everything: the hit test only reports it when
    /// asked to hit-test such elements (`ignorePointerEventsNone: true`).
    click_through_overlay: Option<i64>,
    /// Text nodes → their parent element. Like Chromium, the hit test reports the parent.
    text_parents: HashMap<i64, i64>,
    /// Iframes (see [`Page::frames`]): the tree of the out-of-process frame `F1` (child
    /// session `S1`, shown by `<iframe>` [`OOP_IFRAME`]) ...
    oopif_tree: Option<Value>,
    /// ... and of the same-process frame `F2` (shown by [`SAME_IFRAME`]).
    same_frame_tree: Option<Value>,
    history: Value,
}

/// The page of [`Page::frames`]: a button, an out-of-process and a same-process iframe.
const TOP_BUTTON: i64 = 2;
const OOP_IFRAME: i64 = 3;
const SAME_IFRAME: i64 = 4;
/// In the out-of-process frame (its own DOM numbering: 2 again, like the top button).
const CARD_FIELD: i64 = 2;
const PAY_BUTTON: i64 = 5;
/// In the same-process frame.
const INNER_BUTTON: i64 = 41;
/// Content box of the out-of-process iframe in the page's viewport: x, y, width, height.
const OOP_BOX: (f64, f64, f64, f64) = (300.0, 600.0, 400.0, 100.0);

/// A merged-tree id of a node in the out-of-process frame (slot 1, see `frames.rs`).
fn in_oopif(backend: i64) -> i64 {
    (1 << 32) | backend
}

/// An accessibility tree from `(id, parent, role, name, backend)` rows.
fn ax_rows(rows: &[(&str, &str, &str, &str, i64)]) -> Value {
    let nodes: Vec<Value> = rows
        .iter()
        .map(|(id, parent, role, name, backend)| {
            let children: Vec<&str> = rows.iter().filter(|r| r.1 == *id).map(|r| r.0).collect();
            let mut node = json!({
                "nodeId": id, "role": { "type": "role", "value": role },
                "name": { "type": "computedString", "value": name },
                "childIds": children, "backendDOMNodeId": backend,
            });
            if !parent.is_empty() {
                node["parentId"] = json!(parent);
            }
            node
        })
        .collect();
    json!({ "nodes": nodes })
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
            click_through_overlay: None,
            text_parents: HashMap::new(),
            oopif_tree: None,
            same_frame_tree: None,
            history: json!({ "currentIndex": 0, "entries": [{ "id": 1, "url": "https://acme.com/login" }] }),
        }
    }

    /// A page with a button, a cross-site (out-of-process) iframe holding a card field and a
    /// Pay button, and a same-site iframe holding one button.
    fn frames() -> Page {
        let mut page = Page::login();
        page.tree = ax_rows(&[
            ("1", "", "RootWebArea", "Shop", 1),
            ("2", "1", "button", "Top", TOP_BUTTON),
            ("3", "1", "Iframe", "Payment", OOP_IFRAME),
            ("4", "1", "Iframe", "Same", SAME_IFRAME),
        ]);
        page.tree_after_click = None;
        page.oopif_tree = Some(ax_rows(&[
            ("1", "", "RootWebArea", "Pay", 1),
            ("2", "1", "textbox", "Card", CARD_FIELD),
            ("5", "1", "button", "Pay", PAY_BUTTON),
        ]));
        page.same_frame_tree = Some(ax_rows(&[
            ("1", "", "RootWebArea", "Inner", 40),
            ("2", "1", "button", "Inner", INNER_BUTTON),
        ]));
        // The card field is a text input; the fake ignores sessions here, so the page's
        // button 2 answers like one too (it is only clicked).
        page.probes.insert(
            CARD_FIELD,
            json!({ "connected": true, "tag": "input", "type": "text", "editable": true }),
        );
        page
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
    // Calls in the out-of-process frame's child session carry `sessionId` (FakeBackend).
    let in_frame = params["sessionId"] == "S1";
    let result = match method {
        "Page.getFrameTree" => Ok(json!({ "frameTree": { "frame": { "loaderId": page.loader } } })),
        // Following frames: announce the out-of-process frame while handling the call.
        "Target.setAutoAttach" if page.oopif_tree.is_some() && !in_frame => Ok(json!({
            "__events": [{ "method": "Target.attachedToTarget", "params": {
                "sessionId": "S1",
                "targetInfo": { "targetId": "F1", "type": "iframe", "url": "https://pay.test/card" },
            }}],
        })),
        "Target.setAutoAttach" if page.oopif_tree.is_some() => Ok(json!({})),
        "Accessibility.getFullAXTree" if in_frame => Ok(page.oopif_tree.clone().unwrap()),
        "Accessibility.getFullAXTree" if params["frameId"] == "F2" => {
            Ok(page.same_frame_tree.clone().unwrap())
        }
        "Accessibility.getFullAXTree" => Ok(page.tree.clone()),
        "DOM.describeNode" if backend == OOP_IFRAME && page.oopif_tree.is_some() => Ok(
            json!({ "node": { "frameId": "F1", "attributes": ["src", "https://pay.test/card"] } }),
        ),
        "DOM.describeNode" if backend == SAME_IFRAME && page.same_frame_tree.is_some() => Ok(
            json!({ "node": { "frameId": "F2", "attributes": ["src", "https://acme.com/inner"] } }),
        ),
        "DOM.getBoxModel" if backend == OOP_IFRAME => {
            let (x, y, w, h) = OOP_BOX;
            Ok(json!({ "model": { "content": [x, y, x + w, y, x + w, y + h, x, y + h] } }))
        }
        // Inside the frame: its own coordinates (same layout rule as the page).
        "DOM.getNodeForLocation" if in_frame => {
            Ok(json!({ "backendNodeId": params["y"].as_i64().unwrap_or(0) / 10 }))
        }
        "DOM.getNodeForLocation"
            if page.oopif_tree.is_some() && page.hit_override.is_none() && {
                let (x, y) = (params["x"].as_f64().unwrap(), params["y"].as_f64().unwrap());
                let (bx, by, bw, bh) = OOP_BOX;
                (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y)
            } =>
        {
            // The page's hit test over the frame finds the <iframe> element.
            Ok(json!({ "backendNodeId": OOP_IFRAME }))
        }
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
            let under = page.text_parents.get(&(y / 10)).copied().unwrap_or(y / 10);
            let overlay = page
                .click_through_overlay
                .filter(|_| params["ignorePointerEventsNone"] == true);
            Ok(json!({ "backendNodeId": page.hit_override.or(overlay).unwrap_or(under) }))
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
                // Hit check: does the hit node belong to the target? Like the real script,
                // a text node stands for its parent element when the script normalizes it.
                let hit = id_of(&params["arguments"][0]["objectId"]);
                let normalizes =
                    function.contains("this.nodeType === 1 ? this : this.parentElement");
                let target = match page.text_parents.get(&this) {
                    Some(&parent) if normalizes => parent,
                    _ => this,
                };
                json!(hit == target)
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
        self.snapshot_ref_in(TabId(1), backend).await
    }

    /// [`Harness::snapshot_ref`] in a given tab.
    async fn snapshot_ref_in(&self, tab: TabId, backend: i64) -> String {
        let out = self.ok("page.snapshot", json!({ "tab": tab })).await;
        let text = out["text"].as_str().unwrap().to_string();
        // Look the node up in the snapshot the command just stored (the registry's shared
        // state), instead of guessing ref numbers.
        let tab = self.perception().tab(tab);
        let st = tab.lock().await;
        let (snap, _) = st.history.back().unwrap();
        snap.refs
            .iter()
            .find(|r| r.backend_node_id == backend)
            .map(|r| r.ref_str.clone())
            .unwrap_or_else(|| panic!("no ref for {backend} in:\n{text}"))
    }

    /// The shared per-tab state the commands use.
    fn perception(&self) -> Arc<crate::PerceptionState> {
        self.registry
            .extensions()
            .get::<crate::PerceptionState>()
            .unwrap()
    }

    /// Open another tab showing the same scripted page.
    async fn open_tab(&self) -> TabId {
        self.fake
            .open_tab("https://acme.com/login".into(), true)
            .await
            .unwrap()
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

#[tokio::test(start_paused = true)]
async fn click_through_overlay_does_not_cover_the_target() {
    // Regression: the hit test asked Chromium to hit-test `pointer-events: none` elements,
    // so a decorative overlay that real clicks pass through made every click fail with
    // NOT_INTERACTABLE "covered by ...".
    let mut page = Page::login();
    page.click_through_overlay = Some(COOKIES_DIALOG);
    let h = Harness::new(page);
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    h.ok(
        "page.act",
        json!({ "ref": sign_in, "action": "click", "returns": "none" }),
    )
    .await;
    let hit_test = &h.sent("DOM.getNodeForLocation")[0];
    assert_eq!(hit_test["ignorePointerEventsNone"], false);
    assert_eq!(h.sent("Input.dispatchMouseEvent").len(), 3);
}

#[tokio::test(start_paused = true)]
async fn text_refs_from_find_can_be_clicked_and_hovered() {
    // Regression: a page.find text match is a DOM text node. The hit test reports the
    // element around it, which never "was" the text node, so every click ended in
    // NOT_INTERACTABLE "covered by <its own parent>".
    const COOKIE_TEXT: i64 = 70; // StaticText "We use cookies..." inside paragraph 69
    let mut page = Page::login();
    page.text_parents.insert(COOKIE_TEXT, 69);
    let h = Harness::new(page);
    let out = h.ok("page.find", json!({ "text": "we use cookies" })).await;
    let text = out["text"].as_str().unwrap();
    assert!(text.contains("- text \"We use cookies"), "{text}");
    let r = text
        .split_whitespace()
        .find(|w| w.starts_with('@'))
        .unwrap();
    h.ok(
        "page.act",
        json!({ "ref": r, "action": "hover", "returns": "none" }),
    )
    .await;
    h.ok(
        "page.act",
        json!({ "ref": r, "action": "click", "returns": "none" }),
    )
    .await;
    // The click lands on the text itself (its own box), not the middle of the paragraph.
    let mouse = h.sent("Input.dispatchMouseEvent");
    assert_eq!(mouse.len(), 4);
    assert_eq!(mouse[1]["type"], "mouseMoved");
    assert_eq!(mouse[2]["y"].as_f64(), Some(704.0));
}

#[tokio::test(start_paused = true)]
async fn ref_from_another_tab_never_resolves_and_names_its_tab() {
    // Regression: snapshot ids and e-numbers started at 1 in every tab, so `@1:e5` from
    // tab 1, sent without `tab` (default: the newest tab), clicked e5 of tab 2.
    let mut page = Page::login();
    page.tree_after_click = None;
    let h = Harness::new(page);
    let tab1_ref = h.snapshot_ref(SIGN_IN).await;
    let tab2 = h.open_tab().await;
    let tab2_ref = h.snapshot_ref_in(tab2, SIGN_IN).await;
    // Same page, same DOM ids, but a different snapshot id and e-number.
    assert_ne!(tab1_ref, tab2_ref);
    assert!(tab2_ref.starts_with("@2:"), "{tab2_ref}");

    let bare = tab1_ref.split(':').nth(1).unwrap().to_string();
    for r in [tab1_ref.clone(), bare] {
        let err = h
            .call("page.act", json!({ "ref": r, "action": "click" }))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "{r}: {}", err.message);
        assert!(
            err.message.contains("belongs to tab 1, not to tab 2"),
            "{}",
            err.message
        );
        assert!(err.hint.unwrap().contains("tab=1"));
        assert_eq!(err.retry, bmcp_core::Retry::No);
    }
    assert!(h.sent("Input.dispatchMouseEvent").is_empty());

    // page.read and page.snapshot root= say the same.
    let err = h
        .call("page.read", json!({ "ref": tab1_ref }))
        .await
        .unwrap_err();
    assert!(err.message.contains("belongs to tab 1"), "{}", err.message);
    let err = h
        .call("page.snapshot", json!({ "root": tab1_ref }))
        .await
        .unwrap_err();
    assert!(err.message.contains("belongs to tab 1"), "{}", err.message);

    // With the right tab both refs work.
    h.ok(
        "page.act",
        json!({ "ref": tab1_ref, "tab": 1, "action": "click", "returns": "none" }),
    )
    .await;
    h.ok(
        "page.act",
        json!({ "ref": tab2_ref, "action": "click", "returns": "none" }),
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn closed_tabs_are_forgotten() {
    // Regression: nothing removed a closed tab's state (refs + up to 16 full snapshots).
    let h = Harness::new(Page::login());
    let tab2 = h.open_tab().await;
    h.ok("page.snapshot", json!({ "tab": 1 })).await;
    h.ok("page.snapshot", json!({ "tab": tab2 })).await;
    let mut tracked = h.perception().tracked_tabs();
    tracked.sort();
    assert_eq!(tracked, [TabId(1), tab2]);

    h.fake.close_tab(tab2).await.unwrap();
    h.ok("page.snapshot", json!({ "tab": 1 })).await;
    assert_eq!(h.perception().tracked_tabs(), [TabId(1)]);
}

#[test]
fn snapshot_history_is_capped_in_bytes() {
    use bmcp_perception::{PageHeader, Snapshot, SnapshotOptions};
    let mut state = crate::TabState::default();
    let big = |id: u64| Snapshot {
        id,
        document: 0,
        header: PageHeader::default(),
        text: "x".repeat(3 << 20),
        refs: Vec::new(),
        nodes: Vec::new(),
        skeleton_fallback: false,
        cut: false,
    };
    for id in 1..=10 {
        state.remember(big(id), SnapshotOptions::default());
        assert!(state.history_bytes() <= crate::state::KEEP_HISTORY_BYTES);
    }
    // The newest snapshots stay; a single one over the limit is still kept.
    assert_eq!(state.history.back().unwrap().0.id, 10);
    assert_eq!(state.history.len(), 2);
    let mut huge = big(11);
    huge.text = "x".repeat(crate::state::KEEP_HISTORY_BYTES + 1);
    state.remember(huge, SnapshotOptions::default());
    assert_eq!(state.history.len(), 1);
    assert_eq!(state.latest().unwrap().0.id, 11);
}

#[tokio::test(start_paused = true)]
async fn explicit_refs_stay_valid_over_many_acts_on_an_unchanged_page() {
    // Regression: each page.act took two diff snapshots, and after 64 snapshots the record
    // of the agent's snapshot was gone, so `@1:e5` failed with STALE_REF after ~32 acts.
    let mut page = Page::login();
    page.tree_after_click = None;
    let h = Harness::new(page);
    let email = h.snapshot_ref(EMAIL).await;
    assert!(email.starts_with("@1:"), "{email}");
    for i in 0..40 {
        let out = h
            .call("page.act", json!({ "ref": email, "action": "hover" }))
            .await
            .unwrap_or_else(|e| panic!("act {i} failed: {e:?}"));
        assert!(out["text"].as_str().unwrap().contains("no changes"));
    }
}

// ------------------------------------------------------------------ iframes and shadow DOM

/// The ref printed for `merged` (a node id of the merged tree) in the latest snapshot.
async fn ref_of(h: &Harness, merged: i64) -> String {
    let tab = h.perception().tab(TabId(1));
    let st = tab.lock().await;
    let (snap, _) = st.history.back().unwrap();
    snap.refs
        .iter()
        .find(|r| r.backend_node_id == merged)
        .map(|r| r.ref_str.clone())
        .unwrap_or_else(|| panic!("no ref for {merged} in:\n{}", snap.text))
}

#[tokio::test(start_paused = true)]
async fn snapshot_merges_same_and_out_of_process_iframes() {
    let h = Harness::new(Page::frames());
    let out = h.ok("page.snapshot", json!({})).await;
    let text = out["text"].as_str().unwrap();
    let body: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(
        body,
        [
            "- button \"Top\" @1:e1",
            "- iframe \"Payment\" (pay.test)",
            "  - textbox \"Card\" @1:e2",
            "  - button \"Pay\" @1:e3",
            "- iframe \"Same\" (acme.com)",
            "  - button \"Inner\" @1:e4",
        ],
        "{text}"
    );
    // The out-of-process frame was read in its own session; auto-attach was switched on
    // for the page and for that session (for frames inside it).
    let trees = h.sent("Accessibility.getFullAXTree");
    assert!(trees.iter().any(|p| p["sessionId"] == "S1"), "{trees:?}");
    assert!(trees.iter().any(|p| p["frameId"] == "F2"), "{trees:?}");
    let attach = h.sent("Target.setAutoAttach");
    assert_eq!(attach[0]["flatten"], true);
    assert_eq!(attach[0]["waitForDebuggerOnStart"], false);
    assert!(attach.iter().any(|p| p["sessionId"] == "S1"));

    // page.find sees inside frames too.
    let found = h.ok("page.find", json!({ "name": "pay" })).await;
    assert!(
        found["text"]
            .as_str()
            .unwrap()
            .contains("button \"Pay\" @2:e3"),
        "{found}"
    );
}

#[tokio::test(start_paused = true)]
async fn frame_snapshots_keep_the_worker_pausing_of_identity_tabs() {
    let h = Harness::new(Page::frames());
    // An identity tab asked for paused workers (bmcp-profiles does this before the first
    // navigation) through the shared auto-attach owner.
    let share = h
        .registry
        .extensions()
        .get_or_insert_with(bmcp_cdpshare::CdpShare::default);
    let backend: Arc<dyn BrowserBackend> = h.fake.clone();
    share
        .hold_auto_attach(&backend, TabId(1), None, "identity", true)
        .await
        .unwrap();
    h.ok("page.snapshot", json!({})).await;
    h.ok("page.snapshot", json!({})).await;
    // The first snapshot joined auto-attach on the page session; it was on already, so it had
    // the frames announced again (off + on) — with the pausing kept (it used to send
    // waitForDebuggerOnStart: false). The second snapshot sent nothing.
    let page_calls: Vec<Value> = h
        .sent("Target.setAutoAttach")
        .into_iter()
        .filter(|p| p.get("sessionId").is_none())
        .collect();
    assert_eq!(page_calls.len(), 3, "{page_calls:?}");
    assert_eq!(page_calls[1]["autoAttach"], false);
    for on in [&page_calls[0], &page_calls[2]] {
        assert_eq!(on["autoAttach"], true);
        assert_eq!(on["waitForDebuggerOnStart"], true);
    }
    assert_eq!(
        share.auto_attach_holders(TabId(1), None),
        ["frames", "identity"]
    );
    // The frame's own session still gets plain auto-attach (for frames inside it).
    assert!(
        h.sent("Target.setAutoAttach")
            .iter()
            .any(|p| p["sessionId"] == "S1" && p["waitForDebuggerOnStart"] == false)
    );
    // The out-of-process frame was still merged.
    let out = h.ok("page.snapshot", json!({})).await;
    assert!(out["text"].as_str().unwrap().contains("textbox \"Card\""));
}

#[tokio::test(start_paused = true)]
async fn act_inside_an_out_of_process_iframe_uses_its_session_and_page_coordinates() {
    let h = Harness::new(Page::frames());
    h.ok("page.snapshot", json!({})).await;
    let pay = ref_of(&h, in_oopif(PAY_BUTTON)).await;
    let card = ref_of(&h, in_oopif(CARD_FIELD)).await;
    let top = ref_of(&h, TOP_BUTTON).await;
    // Same DOM node id 2 in two frames, two different refs.
    assert_ne!(card, top);

    h.ok(
        "page.act",
        json!({ "ref": pay, "action": "click", "returns": "none" }),
    )
    .await;
    // Node calls went to the frame's session with the frame's own node id ...
    let resolved = h.sent("DOM.resolveNode");
    assert_eq!(resolved[0]["backendNodeId"], PAY_BUTTON);
    assert_eq!(resolved[0]["sessionId"], "S1");
    // ... the mouse went to the page: the button's centre in the frame (150, 54) plus the
    // iframe's content box corner (300, 600).
    let mouse = h.sent("Input.dispatchMouseEvent");
    assert_eq!(
        (mouse[1]["x"].as_f64(), mouse[1]["y"].as_f64()),
        (Some(450.0), Some(654.0))
    );
    assert!(mouse.iter().all(|p| p.get("sessionId").is_none()));

    h.ok(
        "page.act",
        json!({ "ref": card, "action": "fill", "value": "4242", "returns": "none" }),
    )
    .await;
    let focus = h.sent("DOM.focus");
    assert_eq!(focus[0]["backendNodeId"], CARD_FIELD);
    assert_eq!(focus[0]["sessionId"], "S1");
    assert_eq!(h.sent("Input.insertText")[0]["text"], "4242");

    // The page's own node 2 is still the page's.
    h.ok(
        "page.act",
        json!({ "ref": top, "action": "click", "returns": "none" }),
    )
    .await;
    let last = h.sent("DOM.resolveNode").pop().unwrap();
    assert_eq!(last, json!({ "backendNodeId": TOP_BUTTON }));
}

#[tokio::test(start_paused = true)]
async fn act_inside_a_same_process_iframe_uses_the_page_session() {
    let h = Harness::new(Page::frames());
    h.ok("page.snapshot", json!({})).await;
    let inner = ref_of(&h, INNER_BUTTON).await;
    h.ok(
        "page.act",
        json!({ "ref": inner, "action": "click", "returns": "none" }),
    )
    .await;
    assert_eq!(
        h.sent("DOM.resolveNode")[0],
        json!({ "backendNodeId": INNER_BUTTON })
    );
    // Same-process boxes are already in the page's coordinates.
    let mouse = h.sent("Input.dispatchMouseEvent");
    assert_eq!(
        (mouse[1]["x"].as_f64(), mouse[1]["y"].as_f64()),
        (Some(150.0), Some(414.0))
    );
    assert!(h.sent("DOM.getBoxModel").is_empty());
}

#[tokio::test(start_paused = true)]
async fn page_covering_an_iframe_blocks_clicks_inside_it() {
    let mut page = Page::frames();
    // Something in the page (the top button, say a sticky bar) lies over the frame.
    page.hit_override = Some(TOP_BUTTON);
    let h = Harness::new(page);
    h.ok("page.snapshot", json!({})).await;
    let pay = ref_of(&h, in_oopif(PAY_BUTTON)).await;
    // The frame's own hit test is fine, but the page's finds the cover, not the iframe.
    let err = h
        .call("page.act", json!({ "ref": pay, "action": "click" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(err.message.contains("is covered by"), "{}", err.message);
    assert!(err.message.contains("(450, 654)"), "{}", err.message);
    assert!(h.sent("Input.dispatchMouseEvent").is_empty());
}

#[tokio::test(start_paused = true)]
async fn reading_the_page_walks_shadow_roots() {
    let h = Harness::new(Page::login());
    h.ok("page.read", json!({})).await;
    let evaluated = h.sent("Runtime.evaluate");
    let expression = evaluated
        .iter()
        .filter_map(|p| p["expression"].as_str())
        .find(|e| e.contains("document.body"))
        .unwrap();
    // The page text goes through the same script as one element, which descends into
    // shadow roots (innerText does not).
    assert!(expression.contains(".call(document.body)"), "{expression}");
    assert!(expression.contains("n.shadowRoot"), "{expression}");
    assert!(expression.contains("assignedNodes"), "{expression}");
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
    assert_eq!(
        registry.get("input.humanlike").unwrap().toolset,
        bmcp_core::Toolset::Stealth
    );
}

// ------------------------------------------------------------------ human-like input

/// Mouse events as (type, x, y).
fn mouse_events(h: &Harness) -> Vec<(String, f64, f64)> {
    h.sent("Input.dispatchMouseEvent")
        .iter()
        .map(|p| {
            (
                p["type"].as_str().unwrap().to_string(),
                p["x"].as_f64().unwrap(),
                p["y"].as_f64().unwrap(),
            )
        })
        .collect()
}

async fn human_click(seed: u64) -> (Vec<(String, f64, f64)>, tokio::time::Duration) {
    let h = Harness::new(Page::login());
    let out = h
        .ok("input.humanlike", json!({ "on": true, "seed": seed }))
        .await;
    assert_eq!(out["profile"]["typing_wpm"], 55.0);
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    let started = tokio::time::Instant::now();
    h.ok(
        "page.act",
        json!({ "ref": sign_in, "action": "click", "returns": "none" }),
    )
    .await;
    (mouse_events(&h), started.elapsed())
}

#[tokio::test(start_paused = true)]
async fn humanlike_click_moves_along_a_path_then_presses() {
    let (events, elapsed) = human_click(1).await;
    let moves = events.iter().filter(|e| e.0 == "mouseMoved").count();
    assert!(moves >= 5, "{events:?}");
    let n = events.len();
    assert_eq!(events[n - 2].0, "mousePressed");
    assert_eq!(events[n - 1].0, "mouseReleased");
    // The press happens where the path ended, near the centre (150, 234) of the button.
    let (last_move, press) = (&events[n - 3], &events[n - 2]);
    assert_eq!((last_move.1, last_move.2), (press.1, press.2));
    assert!(
        (press.1 - 150.0).abs() <= 1.5 && (press.2 - 234.0).abs() <= 1.5,
        "{press:?}"
    );
    // It took human time (moving + dwell + hold), not zero.
    assert!(
        elapsed > tokio::time::Duration::from_millis(200),
        "{elapsed:?}"
    );
    // Same seed → same events; different seed → different path.
    assert_eq!(human_click(1).await.0, events);
    assert_ne!(human_click(2).await.0, events);
}

#[tokio::test(start_paused = true)]
async fn humanlike_typing_sends_keys_per_character() {
    let h = Harness::new(Page::login());
    h.ok(
        "input.humanlike",
        json!({ "on": true, "seed": 5, "profile": { "typing_wpm": 120 } }),
    )
    .await;
    let email = h.snapshot_ref(EMAIL).await;
    let started = tokio::time::Instant::now();
    h.ok(
        "page.act",
        json!({ "ref": email, "action": "fill", "value": "Ab é", "returns": "none" }),
    )
    .await;
    // 4 characters at 120 wpm ≈ 100 ms each, plus key hold times.
    assert!(started.elapsed() >= tokio::time::Duration::from_millis(250));
    let keys: Vec<(String, String)> = h
        .sent("Input.dispatchKeyEvent")
        .iter()
        .map(|e| {
            (
                e["type"].as_str().unwrap().to_string(),
                e["key"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("rawKeyDown", "Shift"),
        ("keyDown", "A"),
        ("keyUp", "A"),
        ("keyUp", "Shift"),
        ("keyDown", "b"),
        ("keyUp", "b"),
        ("keyDown", " "),
        ("keyUp", " "),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(keys, expected);
    // "é" has no US key: inserted as text. No one-shot insert of the whole value.
    assert_eq!(h.sent("Input.insertText"), [json!({ "text": "é" })]);
}

#[tokio::test(start_paused = true)]
async fn humanlike_scroll_uses_wheel_notches() {
    let h = Harness::new(Page::login());
    h.ok("input.humanlike", json!({ "on": true, "seed": 9 }))
        .await;
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    h.ok(
        "page.act",
        json!({ "ref": sign_in, "action": "scroll", "value": "down", "returns": "none" }),
    )
    .await;
    let wheels: Vec<Value> = h
        .sent("Input.dispatchMouseEvent")
        .into_iter()
        .filter(|e| e["type"] == "mouseWheel")
        .collect();
    // The fake page reports no scroll size, so 0.8 × the default 600 px view = 480 px.
    assert_eq!(wheels.len(), 5);
    let total: f64 = wheels.iter().map(|w| w["deltaY"].as_f64().unwrap()).sum();
    assert!((total - 480.0).abs() < 1e-6, "{total}");
    // Over the element, after moving the mouse there.
    assert_eq!(
        (wheels[0]["x"].as_f64(), wheels[0]["y"].as_f64()),
        (Some(150.0), Some(234.0))
    );
}

#[tokio::test(start_paused = true)]
async fn humanlike_off_restores_the_fast_path() {
    let h = Harness::new(Page::login());
    h.ok("input.humanlike", json!({ "on": true })).await;
    let out = h.ok("input.humanlike", json!({ "on": false })).await;
    assert_eq!(out["profile"], Value::Null);
    let sign_in = h.snapshot_ref(SIGN_IN).await;
    h.ok(
        "page.act",
        json!({ "ref": sign_in, "action": "click", "returns": "none" }),
    )
    .await;
    let kinds: Vec<String> = mouse_events(&h).into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
}
