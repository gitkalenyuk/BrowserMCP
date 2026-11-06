//! The Rust half of link hints: one [`TabHints`] per tab with the page script installed, and
//! the shared [`HintsService`] (settings + all tabs) in the registry's extensions.
//!
//! **Installing into a tab** (no `Runtime.enable`, which pages can detect):
//!
//! 1. subscribe to the tab's CDP events, `Page.enable` (for `Page.frameNavigated`);
//! 2. `Page.addScriptToEvaluateOnNewDocument {source, worldName: "bmcp-hints"}` — every later
//!    document of the tab runs `hints.js` in our isolated world;
//! 3. [`TabHints::prepare`]: `Page.createIsolatedWorld` (the same name gives the same world
//!    back) → `Runtime.addBinding {executionContextId}` → `Runtime.evaluate` of the source in
//!    that context (a no-op when the document already runs it) + drain the queued events.
//!    A binding added by context *name* never reaches isolated worlds while the Runtime domain
//!    is off (measured for the recorder, `crates/record/src/human.rs`), so step 3 is repeated
//!    for every new document (`Page.frameNavigated` of the main frame).
//!
//! **Events from the page** (`Runtime.bindingCalled`, JSON payload):
//! `{type: "trigger", mode}` (the user typed f / F) → [`HintsService::show`];
//! `{type: "key", key}` while hints are up → [`Filter::key`] → filter / cancel / pick;
//! `{type: "cancel"}` (a mouse click) → forget the hints.
//!
//! A pick hides the labels first, then acts: click and focus go through
//! [`bmcp_browser::perform_on_node`] (the same actionability checks and trusted input as
//! `page.act`), new tab through `BrowserBackend::open_tab`, copy through [`TabHints::copy`].
//! Each pick emits `hints.picked {tab, mode, label, tag, text, href, ok, result | error}`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use bmcp_browser::{Action, PerceptionState, Step, perform_on_node};
use bmcp_core::events::topics;
use bmcp_core::{
    BrowserBackend, Caller, CdpEvents, CmdError, Ctx, DataDir, ErrorCode, EventBus, Extensions,
    TabId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::config::{HintsConfig, Mode};
use crate::filter::{Filter, KeyStep};
use crate::labels::labels;

/// Name of our isolated world (page scripts cannot see it).
pub const WORLD: &str = "bmcp-hints";
/// The CDP binding the page script reports through.
pub const BINDING: &str = "__bmcpHintsEvent";
/// Event emitted after every pick.
pub const PICKED_EVENT: &str = "hints.picked";

const HINTS_JS: &str = include_str!("hints.js");

/// `hints.js` with the settings baked in (each document reads them once at start).
pub fn page_source(config: &HintsConfig) -> String {
    HINTS_JS.replace("__CONFIG__", &config.page_json().to_string())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One raw CDP call.
async fn cdp(ctx: &Ctx, tab: TabId, method: &str, params: Value) -> Result<Value, CmdError> {
    Ok(ctx.backend.cdp(tab, method.to_string(), params).await?)
}

/// What the page script says about one labelled element.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HintItem {
    /// Element tag, e.g. `a`, `button`, `input`.
    pub tag: String,
    /// Accessible-ish text (aria-label, text, placeholder…), at most 60 characters.
    #[serde(default)]
    pub text: String,
    /// Absolute link URL, for links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

impl HintItem {
    /// `a "About us"` (for messages).
    fn describe(&self) -> String {
        if self.text.is_empty() {
            self.tag.clone()
        } else {
            format!("{} \"{}\"", self.tag, self.text)
        }
    }

    /// The link, if it is one a new tab can open (not `javascript:` or `data:`).
    fn openable_href(&self) -> Option<&str> {
        let href = self.href.as_deref()?;
        let lower = href.to_ascii_lowercase();
        ["http://", "https://", "file://", "ftp://"]
            .iter()
            .any(|p| lower.starts_with(p))
            .then_some(href)
    }
}

/// Hints currently on screen in a tab.
struct Shown {
    mode: Mode,
    filter: Filter,
    items: Vec<HintItem>,
}

/// One tab with the page script installed.
pub struct TabHints {
    tab: TabId,
    /// `Page.addScriptToEvaluateOnNewDocument` identifier (replaced when settings change).
    script_id: Mutex<String>,
    /// Source the current documents run (with the settings of when they started).
    source: Mutex<String>,
    /// Execution context of our world in the current document; `None` after a navigation.
    context: Mutex<Option<i64>>,
    /// Hints on screen. An async lock: keys are handled one after another.
    shown: tokio::sync::Mutex<Option<Shown>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TabHints {
    /// Bind the current document: world → binding → script → queued events (JSON texts).
    async fn prepare(&self, ctx: &Ctx) -> Result<Vec<String>, CmdError> {
        let tree = cdp(ctx, self.tab, "Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let world = cdp(
            ctx,
            self.tab,
            "Page.createIsolatedWorld",
            json!({ "frameId": frame, "worldName": WORLD }),
        )
        .await?;
        let context = world["executionContextId"]
            .as_i64()
            .ok_or_else(|| CmdError::internal("Page.createIsolatedWorld gave no context id"))?;
        cdp(
            ctx,
            self.tab,
            "Runtime.addBinding",
            json!({ "name": BINDING, "executionContextId": context }),
        )
        .await?;
        // Start the script (it returns at once if the new-document copy already runs), then
        // collect what it queued before the binding existed.
        let source = lock(&self.source).clone();
        let expression =
            format!("{source}\nglobalThis.__bmcpHints ? globalThis.__bmcpHints.drain() : []");
        let reply = evaluate_raw(ctx, self.tab, context, &expression, true, false).await?;
        *lock(&self.context) = Some(context);
        Ok(reply["value"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Our world's context in the current document (bound on first use after a navigation).
    async fn context(&self, ctx: &Ctx) -> Result<i64, CmdError> {
        if let Some(context) = *lock(&self.context) {
            return Ok(context);
        }
        // Events queued meanwhile stay queued: the navigation handler drains them.
        self.prepare(ctx).await?;
        lock(&self.context).ok_or_else(|| CmdError::internal("no hints context"))
    }

    /// `Runtime.evaluate` in our world; returns the CDP `RemoteObject`. A context that went
    /// away (the page navigated) is looked up again once.
    async fn evaluate(
        &self,
        ctx: &Ctx,
        expression: &str,
        by_value: bool,
        user_gesture: bool,
    ) -> Result<Value, CmdError> {
        let mut retried = false;
        loop {
            let context = self.context(ctx).await?;
            match evaluate_raw(ctx, self.tab, context, expression, by_value, user_gesture).await {
                Err(err) if !retried && context_gone(&err) => {
                    *lock(&self.context) = None;
                    retried = true;
                }
                other => return other,
            }
        }
    }

    /// [`Self::evaluate`] returning the JSON value.
    async fn eval(&self, ctx: &Ctx, expression: &str) -> Result<Value, CmdError> {
        Ok(self.evaluate(ctx, expression, true, false).await?["value"].clone())
    }

    /// Hide the labels in the page (best effort).
    async fn hide_in_page(&self, ctx: &Ctx) {
        let _ = self.eval(ctx, "globalThis.__bmcpHints.hide()").await;
    }

    /// Copy `text` to the clipboard from inside the page, with a user gesture (the clipboard
    /// API needs one). Returns how: `clipboard_api` or `exec_command`.
    pub async fn copy(&self, ctx: &Ctx, text: &str) -> Result<String, CmdError> {
        let expression = format!("globalThis.__bmcpHints.copy({})", json!(text));
        let reply = self.evaluate(ctx, &expression, true, true).await?;
        match reply["value"].as_str() {
            Some("failed") | None => Err(CmdError::new(
                ErrorCode::Internal,
                "the page could not write to the clipboard",
            )
            .with_hint("Make sure the BrowserMCP window is focused, then retry.")),
            Some(how) => Ok(how.to_string()),
        }
    }

    /// Click or focus element `index` of the last collect, like `page.act` does.
    async fn act_on(
        &self,
        ctx: &Ctx,
        index: usize,
        action: Action,
        label: &str,
    ) -> Result<String, CmdError> {
        let expression = format!("globalThis.__bmcpHints.element({index})");
        let object = self.evaluate(ctx, &expression, false, false).await?;
        let Some(object_id) = object["objectId"].as_str() else {
            return Err(CmdError::new(
                ErrorCode::StaleRef,
                format!("{label} is no longer in the page"),
            ));
        };
        let node = cdp(
            ctx,
            self.tab,
            "DOM.describeNode",
            json!({ "objectId": object_id }),
        )
        .await?;
        let _ = cdp(
            ctx,
            self.tab,
            "Runtime.releaseObject",
            json!({ "objectId": object_id }),
        )
        .await;
        let backend_node_id = node["node"]["backendNodeId"]
            .as_i64()
            .ok_or_else(|| CmdError::internal("DOM.describeNode gave no backendNodeId"))?;
        let state = PerceptionState::from_ctx(ctx).tab(self.tab);
        let mut state = state.lock().await;
        let step = Step {
            target: None,
            action,
            value: None,
        };
        perform_on_node(ctx, self.tab, &mut state, backend_node_id, label, &step).await
    }
}

/// The execution context was destroyed (navigation) or is unknown.
fn context_gone(err: &CmdError) -> bool {
    err.message.contains("Cannot find context") || err.message.contains("context was destroyed")
}

/// `Runtime.evaluate {contextId}`; a page exception becomes an error.
async fn evaluate_raw(
    ctx: &Ctx,
    tab: TabId,
    context: i64,
    expression: &str,
    by_value: bool,
    user_gesture: bool,
) -> Result<Value, CmdError> {
    let reply = cdp(
        ctx,
        tab,
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "contextId": context,
            "returnByValue": by_value,
            "awaitPromise": true,
            "userGesture": user_gesture,
        }),
    )
    .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("exception");
        return Err(CmdError::internal(format!("hints script: {text}")));
    }
    Ok(reply["result"].clone())
}

/// Result of [`HintsService::show`].
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ShowOutput {
    pub tab: TabId,
    pub mode: Mode,
    /// Number of labels shown (0 = nothing interactive in view).
    pub count: usize,
    /// Label → element, in page order.
    pub hints: Vec<HintInfo>,
    /// Milliseconds for the whole call (install on first use, find, draw).
    pub ms: f64,
    /// Milliseconds the page spent finding the elements.
    pub page_ms: f64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HintInfo {
    pub label: String,
    #[serde(flatten)]
    pub item: HintItem,
}

/// Settings plus every tab with hints installed. Lives in the registry's extensions.
pub struct HintsService {
    config: Mutex<HintsConfig>,
    /// `data/config/hints.json`; `None` = settings only in memory (tests).
    path: Option<PathBuf>,
    tabs: tokio::sync::Mutex<HashMap<TabId, Arc<TabHints>>>,
}

impl HintsService {
    pub fn new(path: Option<PathBuf>) -> HintsService {
        let config = path.as_deref().map(HintsConfig::load).unwrap_or_default();
        HintsService {
            config: Mutex::new(config),
            path,
            tabs: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The shared instance (created on first use, reading `hints.json` from the `DataDir` in
    /// the extensions when there is one).
    pub fn from_extensions(extensions: &Extensions) -> Arc<HintsService> {
        if let Some(service) = extensions.get::<HintsService>() {
            return service;
        }
        let path = extensions
            .get::<DataDir>()
            .map(|data| HintsConfig::path_in(&data.config()));
        extensions.get_or_insert_with(|| HintsService::new(path))
    }

    pub fn from_ctx(ctx: &Ctx) -> Arc<HintsService> {
        Self::from_extensions(&ctx.extensions)
    }

    pub fn config(&self) -> HintsConfig {
        lock(&self.config).clone()
    }

    /// The tab's hints, installing the page script first if needed.
    pub async fn ensure(
        self: &Arc<Self>,
        ctx: &Ctx,
        tab: TabId,
    ) -> Result<Arc<TabHints>, CmdError> {
        let mut tabs = self.tabs.lock().await;
        if let Some(hints) = tabs.get(&tab) {
            return Ok(hints.clone());
        }
        // Subscribe first, so no event of the new documents is missed.
        let events = ctx.backend.cdp_events(tab).await?;
        cdp(ctx, tab, "Page.enable", json!({})).await?;
        let source = page_source(&self.config());
        let added = cdp(
            ctx,
            tab,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source, "worldName": WORLD }),
        )
        .await?;
        let hints = Arc::new(TabHints {
            tab,
            script_id: Mutex::new(added["identifier"].as_str().unwrap_or_default().to_string()),
            source: Mutex::new(source),
            context: Mutex::new(None),
            shown: tokio::sync::Mutex::new(None),
            task: Mutex::new(None),
        });
        let queued = hints.prepare(ctx).await?;
        // Actions started from the page are the person's, not the API caller's.
        let mut page_ctx = ctx.clone();
        page_ctx.caller = Caller::Ui;
        let task = tokio::spawn(run_tab(
            self.clone(),
            hints.clone(),
            page_ctx,
            events,
            queued,
        ));
        *lock(&hints.task) = Some(task);
        tabs.insert(tab, hints.clone());
        Ok(hints)
    }

    /// The tab's hints if installed.
    pub async fn get(&self, tab: TabId) -> Option<Arc<TabHints>> {
        self.tabs.lock().await.get(&tab).cloned()
    }

    /// Forget a tab (it closed). Stops its event loop.
    pub async fn forget(&self, tab: TabId) {
        if let Some(hints) = self.tabs.lock().await.remove(&tab)
            && let Some(task) = lock(&hints.task).take()
        {
            task.abort();
        }
    }

    /// Tabs with hints installed.
    pub async fn tabs(&self) -> Vec<TabId> {
        let mut tabs: Vec<TabId> = self.tabs.lock().await.keys().copied().collect();
        tabs.sort();
        tabs
    }

    /// Find the interactive elements in view, label them and wait for typing.
    pub async fn show(
        self: &Arc<Self>,
        ctx: &Ctx,
        tab: TabId,
        mode: Mode,
    ) -> Result<ShowOutput, CmdError> {
        let started = Instant::now();
        let hints = self.ensure(ctx, tab).await?;
        self.show_in(ctx, &hints, mode, started).await
    }

    /// [`Self::show`] in a tab that has the script installed. No `ensure` here: the tab's own
    /// event loop calls this for the trigger key (and must not wait on its own installation).
    async fn show_in(
        &self,
        ctx: &Ctx,
        hints: &TabHints,
        mode: Mode,
        started: Instant,
    ) -> Result<ShowOutput, CmdError> {
        let tab = hints.tab;
        let mut shown = hints.shown.lock().await;
        *shown = None;
        let collected = hints.eval(ctx, "globalThis.__bmcpHints.collect()").await?;
        let items: Vec<HintItem> =
            serde_json::from_value(collected["items"].clone()).unwrap_or_default();
        let codes = labels(items.len(), &self.config().alphabet);
        let expression = format!("globalThis.__bmcpHints.render({})", json!(codes));
        hints.eval(ctx, &expression).await?;
        let out = ShowOutput {
            tab,
            mode,
            count: codes.len(),
            hints: codes
                .iter()
                .zip(&items)
                .map(|(label, item)| HintInfo {
                    label: label.clone(),
                    item: item.clone(),
                })
                .collect(),
            ms: round(started.elapsed().as_secs_f64() * 1000.0),
            page_ms: round(collected["ms"].as_f64().unwrap_or(0.0)),
        };
        if !codes.is_empty() {
            *shown = Some(Shown {
                mode,
                filter: Filter::new(codes),
                items,
            });
        }
        Ok(out)
    }

    /// Take the hints off the screen. True when some were shown.
    pub async fn hide(&self, ctx: &Ctx, tab: TabId) -> bool {
        let Some(hints) = self.get(tab).await else {
            return false;
        };
        let was_shown = hints.shown.lock().await.take().is_some();
        hints.hide_in_page(ctx).await;
        was_shown
    }

    /// Type `key` into the hints of `tab`, as if pressed in the page (also used by tests).
    pub async fn key(&self, ctx: &Ctx, tab: TabId, key: &str) -> Result<(), CmdError> {
        let Some(hints) = self.get(tab).await else {
            return Ok(());
        };
        on_key(ctx, &hints, key).await;
        Ok(())
    }

    /// Store new settings and give them to every tab: new documents get a new script, the
    /// current ones are told through `configure`. Turning hints on installs them in `open`.
    pub async fn set_config(
        self: &Arc<Self>,
        ctx: &Ctx,
        config: HintsConfig,
        open: &[TabId],
    ) -> Result<(), CmdError> {
        let was_enabled = {
            let mut current = lock(&self.config);
            let was = current.enabled;
            *current = config.clone();
            was
        };
        if let Some(path) = &self.path {
            config.save(path).map_err(|err| {
                CmdError::internal(format!("cannot write {}: {err}", path.display()))
            })?;
        }
        let source = page_source(&config);
        let configure = format!("globalThis.__bmcpHints.configure({})", config.page_json());
        let installed: Vec<Arc<TabHints>> = self.tabs.lock().await.values().cloned().collect();
        for hints in installed {
            let old = lock(&hints.script_id).clone();
            let _ = cdp(
                ctx,
                hints.tab,
                "Page.removeScriptToEvaluateOnNewDocument",
                json!({ "identifier": old }),
            )
            .await;
            if let Ok(added) = cdp(
                ctx,
                hints.tab,
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": source, "worldName": WORLD }),
            )
            .await
            {
                *lock(&hints.script_id) =
                    added["identifier"].as_str().unwrap_or_default().to_string();
            }
            *lock(&hints.source) = source.clone();
            let _ = hints.eval(ctx, &configure).await;
        }
        if config.enabled && !was_enabled {
            for &tab in open {
                if let Err(err) = self.ensure(ctx, tab).await {
                    tracing::debug!(%tab, %err, "hints: cannot install");
                }
            }
        }
        Ok(())
    }
}

fn round(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

/// The event loop of one tab: page events and navigations, until the tab closes.
async fn run_tab(
    service: Arc<HintsService>,
    hints: Arc<TabHints>,
    ctx: Ctx,
    mut events: CdpEvents,
    queued: Vec<String>,
) {
    for payload in queued {
        on_payload(&service, &ctx, &hints, &payload).await;
    }
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(lost)) => {
                tracing::debug!(tab = %hints.tab, lost, "hints: CDP events lost");
                continue;
            }
            Err(RecvError::Closed) => break, // the tab closed
        };
        if event.session_id.is_some() {
            continue; // iframes and workers
        }
        let p = &event.params;
        match event.method.as_str() {
            "Runtime.bindingCalled" if p["name"] == BINDING => {
                let payload = p["payload"].as_str().unwrap_or_default();
                on_payload(&service, &ctx, &hints, payload).await;
            }
            "Page.frameNavigated" if p["frame"]["parentId"].is_null() => {
                // A new document: old hints are gone with the old one; bind the new one.
                *hints.shown.lock().await = None;
                *lock(&hints.context) = None;
                match hints.prepare(&ctx).await {
                    Ok(queued) => {
                        for payload in queued {
                            on_payload(&service, &ctx, &hints, &payload).await;
                        }
                    }
                    Err(err) => tracing::debug!(tab = %hints.tab, %err, "hints: rebind failed"),
                }
            }
            _ => {}
        }
    }
    let tab = hints.tab;
    service.tabs.lock().await.remove(&tab);
}

/// One event from the page script.
async fn on_payload(service: &Arc<HintsService>, ctx: &Ctx, hints: &Arc<TabHints>, payload: &str) {
    let Ok(event) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    match event["type"].as_str() {
        Some("trigger") => {
            let mode = event["mode"]
                .as_str()
                .and_then(Mode::parse)
                .unwrap_or_default();
            if let Err(err) = service.show_in(ctx, hints, mode, Instant::now()).await {
                tracing::debug!(tab = %hints.tab, %err, "hints: show failed");
            }
        }
        Some("key") => on_key(ctx, hints, event["key"].as_str().unwrap_or_default()).await,
        Some("cancel") => *hints.shown.lock().await = None,
        _ => {}
    }
}

/// A key typed while hints are up.
async fn on_key(ctx: &Ctx, hints: &Arc<TabHints>, key: &str) {
    let mut guard = hints.shown.lock().await;
    let Some(shown) = guard.as_mut() else {
        // The page thinks hints are up but we do not (e.g. hints.hide raced a key).
        drop(guard);
        hints.hide_in_page(ctx).await;
        return;
    };
    match shown.filter.key(key) {
        KeyStep::Ignore => {}
        KeyStep::Typed(typed) => {
            let expression = format!("globalThis.__bmcpHints.filter({})", json!(typed));
            let _ = hints.eval(ctx, &expression).await;
        }
        KeyStep::Cancel => {
            *guard = None;
            drop(guard);
            hints.hide_in_page(ctx).await;
        }
        KeyStep::Pick(index) => {
            let shown = guard.take().expect("checked above");
            drop(guard);
            // Labels off first: they must not be what the click hits.
            hints.hide_in_page(ctx).await;
            pick(ctx, hints, &shown, index).await;
        }
    }
}

/// Do what the mode says with element `index`, and report it on the event bus.
async fn pick(ctx: &Ctx, hints: &Arc<TabHints>, shown: &Shown, index: usize) {
    let item = shown.items.get(index).cloned().unwrap_or_default();
    let code = shown
        .filter
        .labels()
        .get(index)
        .cloned()
        .unwrap_or_default();
    let label = format!("hint {code} {}", item.describe());
    let result = match shown.mode {
        Mode::Newtab => match item.openable_href() {
            Some(url) => ctx
                .backend
                .open_tab(url.to_string(), true)
                .await
                .map(|tab| format!("opened {url} in tab {tab}"))
                .map_err(CmdError::from),
            // Buttons and script links have nothing to open: click them instead.
            None => hints.act_on(ctx, index, Action::Click, &label).await,
        },
        Mode::Copy => {
            let text = item.href.clone().unwrap_or_else(|| item.text.clone());
            hints
                .copy(ctx, &text)
                .await
                .map(|how| format!("copied {text} ({how})"))
        }
        Mode::Focus => hints.act_on(ctx, index, Action::Focus, &label).await,
        Mode::Click => hints.act_on(ctx, index, Action::Click, &label).await,
    };
    let mut data = json!({
        "tab": hints.tab,
        "mode": shown.mode,
        "label": code,
        "tag": item.tag,
        "text": item.text,
        "href": item.href,
        "ok": result.is_ok(),
    });
    match &result {
        Ok(text) => data["result"] = json!(text),
        Err(err) => {
            tracing::info!(tab = %hints.tab, %err, "hints: pick failed");
            data["error"] = json!(err.message);
        }
    }
    ctx.events.emit(PICKED_EVENT, data);
}

/// Install hints in every tab (when `enabled`), now and whenever a tab opens. Call once at
/// startup from inside the tokio runtime; `extensions` is the registry's map.
pub fn start(
    extensions: Arc<Extensions>,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
) -> JoinHandle<()> {
    // Subscribe before returning, so no `tab.created` is missed.
    let mut bus = events.subscribe();
    let mut ctx = Ctx::new(Caller::Ui, backend, events);
    ctx.extensions = extensions;
    tokio::spawn(async move {
        let service = HintsService::from_ctx(&ctx);
        if service.config().enabled
            && let Ok(tabs) = ctx.backend.list_tabs().await
        {
            for tab in tabs {
                spawn_install(&service, &ctx, tab.id);
            }
        }
        loop {
            let event = match bus.recv().await {
                Ok(event) => event,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            };
            match event.topic.as_str() {
                topics::TAB_CREATED if service.config().enabled => {
                    if let Some(id) = event.data["tab"]["id"].as_i64() {
                        spawn_install(&service, &ctx, TabId(id as i32));
                    }
                }
                topics::TAB_CLOSED => {
                    if let Some(id) = event.data["id"].as_i64() {
                        service.forget(TabId(id as i32)).await;
                    }
                }
                _ => {}
            }
        }
    })
}

/// Install in a new tab; its CDP may need a moment to come up, so retry a few times.
fn spawn_install(service: &Arc<HintsService>, ctx: &Ctx, tab: TabId) {
    let (service, ctx) = (service.clone(), ctx.clone());
    tokio::spawn(async move {
        for attempt in 0..5 {
            match service.ensure(&ctx, tab).await {
                Ok(_) => return,
                Err(err) if err.code == ErrorCode::TabNotFound => return,
                Err(err) => {
                    tracing::debug!(%tab, attempt, %err, "hints: install failed, retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        }
    });
}
