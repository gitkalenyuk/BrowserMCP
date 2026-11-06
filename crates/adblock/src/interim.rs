//! INTERIM enforcement over CDP (`adblock.apply`), usable before the CEF hook exists.
//!
//! **This whole module is the temporary path.** The real one is the CEF IO-thread gate
//! (`CefResourceRequestHandler::OnBeforeResourceLoad` → [`crate::ResourceGate`], see
//! README.md), which costs no IPC per request and also covers service workers and OOPIFs.
//! Once the CEF crate calls the gate, `adblock.apply` only keeps the cosmetic part.
//!
//! How it works for one tab:
//! 1. subscribe to the tab's CDP events, then `Fetch.enable` with one pattern
//!    `{urlPattern: "*", requestStage: "Request"}` — every request pauses before it is sent;
//! 2. for each `Fetch.requestPaused` ask the [`Gate`] (filter lists + site policy) and answer
//!    with `Fetch.failRequest {errorReason: "BlockedByClient"}`, `Fetch.fulfillRequest`
//!    (redirect) or `Fetch.continueRequest`. Every paused request is always answered: if the
//!    block answer fails we still try to continue it, and if events were lost (`Lagged`) we
//!    toggle `Fetch.disable`/`Fetch.enable`, which releases everything still paused;
//! 3. cosmetic filtering: the host's hide CSS is registered with
//!    `Page.addScriptToEvaluateOnNewDocument` in an isolated world, so every later document of
//!    that host gets it at document start. When the main frame commits a new page
//!    (`Page.frameNavigated`), that script is replaced for the new host and the CSS is also
//!    injected into the new page right away; on `DOMContentLoaded` the generic class/id rules
//!    follow. Injection uses `Page.createIsolatedWorld` + `Runtime.evaluate {contextId}`; no
//!    `DOM.*` and no `Runtime.enable`;
//! 4. site policy `js: false` → `Emulation.setScriptExecutionDisabled`, `animations: false`
//!    → CSS that stops animations + `prefers-reduced-motion: reduce` (both from the commit on).
//!
//! **Never wait for a renderer-handled CDP command (`Page.*Script*`, `Runtime.*`,
//! `Emulation.*`) before answering a paused request.** While a main-frame navigation is
//! pending, Chromium holds back messages to the renderer until the navigation commits — and
//! it cannot commit while we keep its request paused: a 30 s CDP timeout each time (seen
//! live). `Fetch.*` is handled in the browser process and always answers.
//!
//! The decision is made right in the event loop (it is synchronous and takes microseconds);
//! the CDP answers are sent from spawned tasks so a slow answer never delays the next event.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bmcp_core::{BackendError, BrowserBackend, CdpEvent, TabId};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::engine::{ResourceType, hide_css};
use crate::gate::{Gate, GateAction, GateDecision, RequestInfo, ResourceGate};

/// Name of our isolated world (the page's own scripts cannot see it).
pub const WORLD: &str = "bmcp-adblock";

/// CSS that stops animations and transitions (site policy `animations: false`).
const NO_ANIMATIONS_CSS: &str = "*,*::before,*::after{animation:none!important;\
transition:none!important;scroll-behavior:auto!important}\n";

/// Registered at document start (isolated world): add the host's hide CSS as early as
/// possible. Skips frames of other hosts (the script runs in every frame of the tab).
const DOC_START_JS: &str = r#"(() => {
  const host = __HOST__;
  if (location.hostname !== host) return;
  const css = __CSS__;
  const add = () => {
    const root = document.documentElement;
    if (!root) return false;
    if (!document.getElementById("bmcp-adblock-css")) {
      const style = document.createElement("style");
      style.id = "bmcp-adblock-css";
      style.textContent = css;
      root.appendChild(style);
    }
    return true;
  };
  if (!add()) {
    const observer = new MutationObserver(() => { if (add()) observer.disconnect(); });
    observer.observe(document, { childList: true });
  }
})();"#;

/// Put CSS into the current document (style element `__ID__`), and with `__COLLECT__` return
/// the classes and ids used on the page (for generic hiding rules).
const INJECT_JS: &str = r#"(() => {
  const id = __ID__, css = __CSS__;
  let style = document.getElementById(id);
  if (css) {
    if (!style) {
      style = document.createElement("style");
      style.id = id;
      (document.head || document.documentElement).appendChild(style);
    }
    if (style.textContent !== css) style.textContent = css;
  }
  if (!__COLLECT__) return null;
  const classes = new Set(), ids = new Set();
  for (const el of document.querySelectorAll("[class],[id]")) {
    if (el.id) ids.add(el.id);
    for (const c of el.classList) classes.add(c);
    if (classes.size + ids.size > 20000) break;
  }
  return { classes: [...classes], ids: [...ids] };
})()"#;

/// Remove everything we injected into the current document.
const REMOVE_JS: &str = r#"for (const id of ["bmcp-adblock-css", "bmcp-adblock-generic"]) {
  const el = document.getElementById(id); if (el) el.remove();
}"#;

/// One tab's interception: the event-loop task and its shared state.
pub(crate) struct Session {
    pub task: tokio::task::JoinHandle<()>,
    pub shared: Arc<Shared>,
}

/// State shared by a tab's event loop and the tasks it spawns.
pub(crate) struct Shared {
    tab: TabId,
    backend: Arc<dyn BrowserBackend>,
    gate: Arc<Gate>,
    /// Main frame id (= target id); `Document` requests of this frame are navigations.
    main_frame: Mutex<String>,
    /// URL of the current top-level document: the "source" of subresource requests.
    page_url: Mutex<String>,
    /// Identifier of our `Page.addScriptToEvaluateOnNewDocument` script.
    script_id: Mutex<Option<String>>,
    /// We disabled script execution (`js: false`).
    js_disabled: AtomicBool,
    /// We emulate `prefers-reduced-motion: reduce` (`animations: false`).
    reduced_motion: AtomicBool,
    pub metrics: Metrics,
}

/// Overhead and outcome counters of one interception session.
#[derive(Default)]
pub(crate) struct Metrics {
    started: Mutex<Option<Instant>>,
    paused: AtomicU64,
    blocked: AtomicU64,
    fulfilled: AtomicU64,
    continued: AtomicU64,
    /// CDP answers that failed (request gone, tab navigated away…).
    errors: AtomicU64,
    /// Events lost because the loop fell behind (then Fetch was reset).
    lagged: AtomicU64,
    decision_ns: AtomicU64,
    decision_ns_max: AtomicU64,
    roundtrip_us: AtomicU64,
    roundtrip_us_max: AtomicU64,
}

/// `adblock.stats` view of a session.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct InterimStats {
    /// The interception task is running.
    pub active: bool,
    /// Paused requests seen.
    pub paused: u64,
    /// Answered with `Fetch.failRequest`.
    pub blocked: u64,
    /// Answered with `Fetch.fulfillRequest` (redirect resources).
    pub fulfilled: u64,
    /// Answered with `Fetch.continueRequest`.
    pub continued: u64,
    pub errors: u64,
    pub lagged: u64,
    /// Average time of the gate decision itself (filter lists + policy), microseconds.
    pub avg_decision_us: f64,
    pub max_decision_us: f64,
    /// Average time from `requestPaused` to our answer being acknowledged: the latency the
    /// interception adds to every request, milliseconds.
    pub avg_roundtrip_ms: f64,
    pub max_roundtrip_ms: f64,
    pub uptime_s: u64,
}

impl Shared {
    pub(crate) fn stats(&self, active: bool) -> InterimStats {
        let m = &self.metrics;
        let paused = m.paused.load(Ordering::Relaxed);
        let answered = m.blocked.load(Ordering::Relaxed)
            + m.fulfilled.load(Ordering::Relaxed)
            + m.continued.load(Ordering::Relaxed);
        let avg = |total: u64, n: u64| if n == 0 { 0.0 } else { total as f64 / n as f64 };
        InterimStats {
            active,
            paused,
            blocked: m.blocked.load(Ordering::Relaxed),
            fulfilled: m.fulfilled.load(Ordering::Relaxed),
            continued: m.continued.load(Ordering::Relaxed),
            errors: m.errors.load(Ordering::Relaxed),
            lagged: m.lagged.load(Ordering::Relaxed),
            avg_decision_us: avg(m.decision_ns.load(Ordering::Relaxed), paused) / 1000.0,
            max_decision_us: m.decision_ns_max.load(Ordering::Relaxed) as f64 / 1000.0,
            avg_roundtrip_ms: avg(m.roundtrip_us.load(Ordering::Relaxed), answered) / 1000.0,
            max_roundtrip_ms: m.roundtrip_us_max.load(Ordering::Relaxed) as f64 / 1000.0,
            uptime_s: lock(&m.started).map_or(0, |t| t.elapsed().as_secs()),
        }
    }

    fn page_url(&self) -> String {
        lock(&self.page_url).clone()
    }

    async fn cdp(&self, method: &str, params: Value) -> Result<Value, BackendError> {
        self.backend.cdp(self.tab, method.to_string(), params).await
    }
}

/// What `adblock.apply {on: true}` set up.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ApplyReport {
    /// URL of the page shown when interception started.
    pub page_url: String,
    /// Hide selectors injected into that page (host-specific + generic).
    pub hidden_selectors: usize,
    /// Generic class/id selectors among them.
    pub generic_selectors: usize,
    /// JavaScript was disabled by the site policy.
    pub js_disabled: bool,
    /// Animations were disabled by the site policy.
    pub animations_disabled: bool,
    /// Where requests are blocked: "cef" (the browser's network hook, every request incl.
    /// workers and iframes) or "cdp" (interim `Fetch` interception of this tab).
    pub network: &'static str,
}

/// Start intercepting `tab`. The caller checks that no session runs yet.
pub(crate) async fn start(
    backend: Arc<dyn BrowserBackend>,
    gate: Arc<Gate>,
    tab: TabId,
) -> Result<(Session, ApplyReport), BackendError> {
    // Subscribe before enabling Fetch, so no paused request can be missed.
    let events = backend.cdp_events(tab).await?;
    let tree = backend
        .cdp(tab, "Page.getFrameTree".into(), json!({}))
        .await?;
    let frame = &tree["frameTree"]["frame"];
    let shared = Arc::new(Shared {
        tab,
        backend: backend.clone(),
        gate,
        main_frame: Mutex::new(frame["id"].as_str().unwrap_or_default().to_string()),
        page_url: Mutex::new(frame["url"].as_str().unwrap_or_default().to_string()),
        script_id: Mutex::new(None),
        js_disabled: AtomicBool::new(false),
        reduced_motion: AtomicBool::new(false),
        metrics: Metrics::default(),
    });
    *lock(&shared.metrics.started) = Some(Instant::now());
    // Page events: DOMContentLoaded (generic cosmetic pass) and frameNavigated (page URL).
    shared.cdp("Page.enable", json!({})).await?;
    // The loop must already run when the first request pauses (the cosmetic calls below may
    // wait for a navigation, which waits for its paused request).
    let task = tokio::spawn(event_loop(shared.clone(), events));
    // With the CEF network hook (`Gate::is_native`) every request is already decided in the
    // browser's network stack: only the cosmetic part below is needed here.
    let native = shared.gate.is_native();
    if !native && let Err(err) = shared.cdp("Fetch.enable", fetch_patterns()).await {
        task.abort();
        return Err(err);
    }

    let url = shared.page_url();
    prepare_document(&shared, &url).await;
    let (hidden, generic) = inject_current(&shared, &url).await;
    let report = ApplyReport {
        page_url: url,
        hidden_selectors: hidden + generic,
        generic_selectors: generic,
        js_disabled: shared.js_disabled.load(Ordering::Relaxed),
        animations_disabled: shared.reduced_motion.load(Ordering::Relaxed),
        network: if native { "cef" } else { "cdp" },
    };
    Ok((Session { task, shared }, report))
}

/// Stop intercepting and undo what we changed in the tab (best effort: the tab may be gone).
pub(crate) async fn stop(session: Session) {
    session.task.abort();
    let shared = session.shared;
    // Disabling Fetch also releases any request that is still paused.
    let _ = shared.cdp("Fetch.disable", json!({})).await;
    let old = lock(&shared.script_id).take();
    if let Some(identifier) = old {
        let _ = shared
            .cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({ "identifier": identifier }),
            )
            .await;
    }
    if shared.js_disabled.swap(false, Ordering::Relaxed) {
        let _ = shared
            .cdp(
                "Emulation.setScriptExecutionDisabled",
                json!({ "value": false }),
            )
            .await;
    }
    if shared.reduced_motion.swap(false, Ordering::Relaxed) {
        let _ = shared
            .cdp("Emulation.setEmulatedMedia", json!({ "features": [] }))
            .await;
    }
    let _ = evaluate_isolated(&shared, REMOVE_JS).await;
}

fn fetch_patterns() -> Value {
    json!({ "patterns": [{ "urlPattern": "*", "requestStage": "Request" }] })
}

/// Read the tab's events until it closes (or the session is stopped).
async fn event_loop(shared: Arc<Shared>, mut events: bmcp_core::CdpEvents) {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(lost)) => {
                // Lost `requestPaused` events would leave requests hanging forever:
                // re-enabling Fetch releases everything that is still paused.
                tracing::warn!(tab = %shared.tab, lost, "adblock: CDP events lost; resetting Fetch");
                shared.metrics.lagged.fetch_add(lost, Ordering::Relaxed);
                if shared.gate.is_native() {
                    continue; // Fetch is not enabled: nothing can be left paused
                }
                let shared = shared.clone();
                tokio::spawn(async move {
                    let _ = shared.cdp("Fetch.disable", json!({})).await;
                    let _ = shared.cdp("Fetch.enable", fetch_patterns()).await;
                });
                continue;
            }
            Err(RecvError::Closed) => break, // tab closed
        };
        handle_event(&shared, event);
    }
}

fn handle_event(shared: &Arc<Shared>, event: CdpEvent) {
    // Events of child sessions (OOPIFs, workers) are not ours: we only enabled Fetch on the
    // page target, and could not answer in another session anyway.
    if event.session_id.is_some() {
        return;
    }
    match event.method.as_str() {
        "Fetch.requestPaused" => on_request_paused(shared, &event.params),
        "Page.domContentEventFired" => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let url = shared.page_url();
                inject_current(&shared, &url).await;
            });
        }
        "Page.frameNavigated" => {
            let frame = &event.params["frame"];
            if frame.get("parentId").is_none()
                && let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str())
            {
                *lock(&shared.main_frame) = id.to_string();
                *lock(&shared.page_url) = url.to_string();
                // The new page has committed: renderer commands work again (module docs).
                let shared = shared.clone();
                let url = url.to_string();
                tokio::spawn(async move {
                    prepare_document(&shared, &url).await;
                    inject_current(&shared, &url).await;
                });
            }
        }
        _ => {}
    }
}

/// Decide now; answer from a spawned task.
fn on_request_paused(shared: &Arc<Shared>, params: &Value) {
    let received = Instant::now();
    let metrics = &shared.metrics;
    metrics.paused.fetch_add(1, Ordering::Relaxed);
    let Some(request_id) = params["requestId"].as_str().map(str::to_string) else {
        return;
    };
    let url = params["request"]["url"].as_str().unwrap_or_default();
    // We only intercept at the request stage; anything else is just let through.
    let response_stage =
        params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some();
    let is_main_frame = params["resourceType"] == "Document"
        && params["frameId"].as_str() == Some(lock(&shared.main_frame).as_str());
    let resource_type =
        ResourceType::from_cdp(params["resourceType"].as_str().unwrap_or(""), is_main_frame);

    let decision = if response_stage {
        GateDecision::allow()
    } else {
        let source = if is_main_frame {
            url.to_string()
        } else {
            shared.page_url()
        };
        let started = Instant::now();
        let decision = shared.gate.on_request(&RequestInfo {
            tab: Some(shared.tab),
            url,
            source_url: &source,
            resource_type,
        });
        let ns = started.elapsed().as_nanos() as u64;
        metrics.decision_ns.fetch_add(ns, Ordering::Relaxed);
        metrics.decision_ns_max.fetch_max(ns, Ordering::Relaxed);
        decision
    };
    // Subresources that follow belong to the new page.
    if is_main_frame {
        *lock(&shared.page_url) = url.to_string();
    }

    // Only browser-side `Fetch.*` calls from here on: see the module docs on why we must not
    // touch the renderer while a navigation request is paused.
    let shared = shared.clone();
    tokio::spawn(async move {
        answer(&shared, &request_id, &decision).await;
        let us = received.elapsed().as_micros() as u64;
        shared.metrics.roundtrip_us.fetch_add(us, Ordering::Relaxed);
        shared
            .metrics
            .roundtrip_us_max
            .fetch_max(us, Ordering::Relaxed);
    });
}

/// Resolve one paused request according to the decision. Never leaves it paused if CDP lets
/// us answer at all.
async fn answer(shared: &Shared, request_id: &str, decision: &GateDecision) {
    let metrics = &shared.metrics;
    let first = match decision.action {
        GateAction::Block => shared
            .cdp(
                "Fetch.failRequest",
                json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
            )
            .await
            .map(|_| &metrics.blocked),
        GateAction::Redirect => match decision.redirect.as_deref().and_then(fulfill_params) {
            Some(mut params) => {
                params["requestId"] = json!(request_id);
                shared
                    .cdp("Fetch.fulfillRequest", params)
                    .await
                    .map(|_| &metrics.fulfilled)
            }
            // A redirect we cannot express: block instead.
            None => shared
                .cdp(
                    "Fetch.failRequest",
                    json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
                )
                .await
                .map(|_| &metrics.blocked),
        },
        GateAction::Allow => Err(BackendError::Cdp(String::new())), // continue below
    };
    let counter = match first {
        Ok(counter) => Some(counter),
        Err(_) => shared
            .cdp("Fetch.continueRequest", json!({ "requestId": request_id }))
            .await
            .ok()
            .map(|_| &metrics.continued),
    };
    match counter {
        Some(counter) => counter.fetch_add(1, Ordering::Relaxed),
        // The request is gone (navigation, tab closed): nothing left to resolve.
        None => metrics.errors.fetch_add(1, Ordering::Relaxed),
    };
}

/// `Fetch.fulfillRequest` parameters for a `data:<mime>;base64,<body>` redirect.
fn fulfill_params(data_url: &str) -> Option<Value> {
    let rest = data_url.strip_prefix("data:")?;
    let (mime, body) = rest.split_once(";base64,")?;
    Some(json!({
        "responseCode": 200,
        "responseHeaders": [{ "name": "Content-Type", "value": mime }],
        "body": body,
    }))
}

/// Everything about a page that must be in place before its document loads: the cosmetic
/// script for new documents and the JS / motion emulation of its site policy.
async fn prepare_document(shared: &Shared, url: &str) {
    let css = page_css(shared, url);
    let host = crate::engine::host_and_domain(url).map(|(host, _)| host);

    let old = lock(&shared.script_id).take();
    if let Some(identifier) = old {
        let _ = shared
            .cdp(
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({ "identifier": identifier }),
            )
            .await;
    }
    if let (Some(host), false) = (host, css.is_empty()) {
        let source = DOC_START_JS
            .replace("__HOST__", &json!(host).to_string())
            .replace("__CSS__", &json!(css).to_string());
        let reply = shared
            .cdp(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": source, "worldName": WORLD }),
            )
            .await;
        if let Ok(reply) = reply {
            *lock(&shared.script_id) = reply["identifier"].as_str().map(str::to_string);
        }
    }

    let policy = shared
        .gate
        .evaluate(&RequestInfo {
            tab: Some(shared.tab),
            url,
            source_url: url,
            resource_type: ResourceType::MainFrame,
        })
        .policy;
    let disable_js = !policy.js;
    if shared.js_disabled.swap(disable_js, Ordering::Relaxed) != disable_js {
        let _ = shared
            .cdp(
                "Emulation.setScriptExecutionDisabled",
                json!({ "value": disable_js }),
            )
            .await;
    }
    let reduce = !policy.animations;
    if shared.reduced_motion.swap(reduce, Ordering::Relaxed) != reduce {
        let features = if reduce {
            json!([{ "name": "prefers-reduced-motion", "value": "reduce" }])
        } else {
            json!([])
        };
        let _ = shared
            .cdp(
                "Emulation.setEmulatedMedia",
                json!({ "features": features }),
            )
            .await;
    }
}

/// Hide CSS (filter lists) + no-animation CSS (site policy) for a page.
fn page_css(shared: &Shared, url: &str) -> String {
    let mut css = shared
        .gate
        .cosmetic(url)
        .map(|c| c.style)
        .unwrap_or_default();
    let policy = shared
        .gate
        .evaluate(&RequestInfo {
            tab: None,
            url,
            source_url: url,
            resource_type: ResourceType::MainFrame,
        })
        .policy;
    if !policy.animations {
        css.push_str(NO_ANIMATIONS_CSS);
    }
    css
}

/// Inject the page CSS into the document shown now, then the generic class/id rules for
/// the classes and ids it uses. Returns `(host-specific selectors, generic selectors)`.
async fn inject_current(shared: &Shared, url: &str) -> (usize, usize) {
    let cosmetic = shared.gate.cosmetic(url);
    let css = page_css(shared, url);
    let collect = cosmetic.as_ref().is_some_and(|c| !c.generichide);
    let script = inject_script("bmcp-adblock-css", &css, collect);
    let Ok(found) = evaluate_isolated(shared, &script).await else {
        return (0, 0);
    };
    let hidden = cosmetic.as_ref().map_or(0, |c| c.hide_selectors.len());
    let Some(cosmetic) = cosmetic.filter(|_| collect) else {
        return (hidden, 0);
    };
    let strings = |key: &str| -> Vec<String> {
        found[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (classes, ids) = (strings("classes"), strings("ids"));
    let generic = shared
        .gate
        .with_filter(|f| f.generic_selectors(&classes, &ids, &cosmetic.exceptions))
        .unwrap_or_default();
    if !generic.is_empty() {
        let script = inject_script("bmcp-adblock-generic", &hide_css(&generic), false);
        let _ = evaluate_isolated(shared, &script).await;
    }
    (hidden, generic.len())
}

/// [`INJECT_JS`] for style element `id`. The CSS goes in last, so text inside it can never
/// be mistaken for a placeholder.
fn inject_script(id: &str, css: &str, collect: bool) -> String {
    INJECT_JS
        .replace("__ID__", &json!(id).to_string())
        .replace("__COLLECT__", if collect { "true" } else { "false" })
        .replace("__CSS__", &json!(css).to_string())
}

/// Run `expression` in our isolated world of the main frame; returns its JSON value.
async fn evaluate_isolated(shared: &Shared, expression: &str) -> Result<Value, BackendError> {
    let frame_id = lock(&shared.main_frame).clone();
    let world = shared
        .cdp(
            "Page.createIsolatedWorld",
            json!({ "frameId": frame_id, "worldName": WORLD }),
        )
        .await?;
    let context_id = world["executionContextId"].clone();
    let reply = shared
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": expression, "contextId": context_id, "returnByValue": true }),
        )
        .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        tracing::debug!(%details, "adblock: injected script failed");
        return Err(BackendError::Cdp("injected script failed".into()));
    }
    Ok(reply["result"]["value"].clone())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
