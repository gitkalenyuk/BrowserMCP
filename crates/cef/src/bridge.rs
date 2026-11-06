//! Bridge between async Rust (tokio) and the CEF UI thread.
//!
//! Rules (see docs/plan.md, "Правило мосту"):
//! * CEF objects (`Browser`, `Registration`, ...) live only on the UI thread, inside the
//!   thread-local [`UiState`]. Other threads refer to tabs by [`TabId`] only.
//! * Async code reaches the UI thread with [`run_on_ui`], which posts a CEF task and
//!   awaits the closure's return value through a oneshot channel.
//! * Never hold the `UiState` borrow while calling into CEF: CEF may call our handlers
//!   back synchronously, and they need the borrow too. Use [`with_ui`] for short
//!   lookups, copy what you need out, then call CEF.
//!
//! Tab lifecycle (see also `chrome/mod.rs`):
//! 1. Someone wants a tab: we create a content `BrowserView`, give it a unique view id and
//!    remember a [`PendingTab`] under that id (which window, where in the strip, activate?).
//! 2. CEF creates the browser when the view is added to a window and calls
//!    `on_after_created` → [`register_tab`]: the tab gets its [`TabEntry`], is put into its
//!    window's `TabStrip`, `tab.created` is emitted and the opener is told the new id.
//! 3. `on_before_close` → [`unregister_tab`]: entry and strip slot are removed,
//!    `tab.closed` is emitted and, if it was the shown tab, its neighbour is shown.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use bmcp_cdp::{BoxFuture, CdpClient, CdpError, Transport};
use bmcp_core::events::{tab as tab_events, topics};
use bmcp_core::{BackendError, EventBus, SplitSide, TabId, TabInfo, TabSplit};
use cef::*;
use serde_json::Value;
use tokio::sync::{oneshot, watch};

use crate::profiles::ProfileKey;

/// Load progress of one tab. `seq` increases every time a load finishes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LoadState {
    pub seq: u64,
    pub loading: bool,
}

pub(crate) struct TabEntry {
    pub browser: Browser,
    pub cdp: Arc<CdpClient>,
    /// Keeps the DevTools observer attached; dropping it detaches.
    pub _devtools: Option<Registration>,
    pub title: String,
    pub url: String,
    pub favicon: Option<String>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// Key of the composed window that shows this tab (None in the single layout and for
    /// off-screen tabs).
    pub window: Option<i32>,
    pub load: watch::Sender<LoadState>,
    /// Profile (request context) of the tab.
    pub profile: ProfileKey,
    /// Mass run the tab belongs to (downloads go to its folder).
    pub run: Option<String>,
    /// Windowless (off-screen rendering) tab.
    pub offscreen: bool,
    /// Page zoom in percent (see `zoom.rs`).
    pub zoom: f64,
}

impl TabEntry {
    /// The tab as the API sees it. `split` = its half of a split view, if any.
    pub fn info(&self, id: TabId, active: bool, split: Option<TabSplit>) -> TabInfo {
        TabInfo {
            id,
            title: self.title.clone(),
            url: self.url.clone(),
            loading: self.load.borrow().loading,
            favicon: self.favicon.clone(),
            can_go_back: self.can_go_back,
            can_go_forward: self.can_go_forward,
            active,
            group_id: None,
            locked_by: None,
            profile: Some(self.profile.name()),
            incognito: self.profile.incognito,
            offscreen: self.offscreen,
            window: self.window,
            split,
            zoom: crate::zoom::badge(self.zoom),
        }
    }
}

/// A tab that was asked for but whose browser does not exist yet. Keyed by the content
/// view's id in [`UiState::pending`].
pub(crate) struct PendingTab {
    /// Composed window it goes to (None in the single layout).
    pub window: Option<i32>,
    /// Position in the tab strip (None = end).
    pub index: Option<usize>,
    pub activate: bool,
    /// What it will load; shown until the page reports its real URL.
    pub url: String,
    /// Told the new tab id once the browser exists (`tabs.open` waits for it).
    pub reply: Option<oneshot::Sender<TabId>>,
    pub profile: ProfileKey,
    pub run: Option<String>,
    /// The browser starts on `about:blank`; this URL is loaded once the tab was prepared
    /// (identity applied, see `hooks.rs`).
    pub then_load: Option<String>,
    /// Session restore: (restore tag, saved index) — see `session.rs`.
    pub restore: Option<(u32, usize)>,
}

impl PendingTab {
    /// A tab that shows `url` in `profile`. When the profile wants its tabs prepared, the
    /// browser must be created with [`PendingTab::initial_url`] instead.
    pub fn new(url: &str, profile: ProfileKey, run: Option<String>) -> Self {
        let then_load = crate::hooks::wants_prepare(&profile.name()).then(|| url.to_owned());
        PendingTab {
            window: None,
            index: None,
            activate: true,
            url: url.to_owned(),
            reply: None,
            profile,
            run,
            then_load,
            restore: None,
        }
    }

    /// What the browser is created with.
    pub fn initial_url(&self) -> &str {
        if self.then_load.is_some() {
            "about:blank"
        } else {
            &self.url
        }
    }
}

/// Everything the shell needs to open another window later.
#[derive(Clone)]
pub(crate) struct ShellContext {
    pub style: RuntimeStyle,
    pub layout: crate::WindowLayout,
    pub browsers: Arc<crate::shell::Browsers>,
}

#[derive(Default)]
pub(crate) struct UiState {
    /// Web pages only. This is what `list_tabs` / MCP see.
    pub tabs: BTreeMap<TabId, TabEntry>,
    /// Our own UI and palette browsers, by browser id. Never tabs.
    pub chrome_views: BTreeMap<i32, crate::chrome::ChromeView>,
    /// Composed windows, by window key (= the Window's view id).
    pub windows: BTreeMap<i32, crate::chrome::ChromeWindow>,
    /// Tabs being created, by content view id.
    pub pending: BTreeMap<i32, PendingTab>,
    /// Popup about to be created by this opener browser id: (stays in the background?,
    /// target URL). Set in `on_before_popup`, used in `on_popup_browser_view_created`.
    pub popups: BTreeMap<i32, (bool, String)>,
    /// Key of the window that was active last (where `tabs.open` puts new tabs).
    pub last_window: Option<i32>,
    pub shell: Option<ShellContext>,
    /// Request contexts and settings of the profiles.
    pub profiles: crate::profiles::ProfilesState,
    /// DevTools browsers (by browser id): neither tabs nor chrome views.
    pub devtools: BTreeMap<i32, Browser>,
    /// Permission requests waiting for the permission bar, by prompt id.
    pub prompts: BTreeMap<u64, crate::prompts::Waiting>,
    /// Closed-tabs stack and session restore progress.
    pub session: crate::session::SessionState,
}

thread_local! {
    static UI: RefCell<UiState> = RefCell::new(UiState::default());
}

/// Short, non-reentrant access to the UI state. UI thread only.
pub(crate) fn with_ui<T>(f: impl FnOnce(&mut UiState) -> T) -> T {
    debug_assert_ne!(currently_on(ThreadId::UI), 0, "with_ui off the UI thread");
    UI.with(|ui| f(&mut ui.borrow_mut()))
}

/// Ids for content views and windows (0 is the default view id, so start at 1).
pub(crate) fn next_view_id() -> i32 {
    use std::sync::atomic::{AtomicI32, Ordering};
    static NEXT: AtomicI32 = AtomicI32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

// ------------------------------------------------------------------ events

static EVENTS: OnceLock<EventBus> = OnceLock::new();

/// The bus `tab.*` events go to (shared with the API server). Set once at startup.
pub(crate) fn set_events(bus: EventBus) {
    let _ = EVENTS.set(bus);
}

pub(crate) fn emit(topic: &str, data: Value) {
    if let Some(bus) = EVENTS.get() {
        bus.emit(topic, data);
    }
    // Every tab change goes through here: the session file follows (debounced).
    if topic.starts_with("tab.") {
        crate::session::changed();
    }
}

/// Is `id` the shown tab of its window? (Single-layout tabs are always "active".)
fn is_active(ui: &UiState, id: TabId, window: Option<i32>) -> bool {
    match window.and_then(|key| ui.windows.get(&key)) {
        Some(w) => w.strip.active() == Some(id),
        None => true,
    }
}

/// Tab `id`'s half of its window's split view, if it is in one.
fn split_of(ui: &UiState, id: TabId, window: Option<i32>) -> Option<TabSplit> {
    let split = ui.windows.get(&window?)?.split?;
    let (side, other) = if split.left == id {
        (SplitSide::Left, split.right)
    } else if split.right == id {
        (SplitSide::Right, split.left)
    } else {
        return None;
    };
    Some(TabSplit {
        side,
        other,
        ratio: split.ratio,
    })
}

/// Current info of one tab. UI thread.
pub(crate) fn tab_info(id: TabId) -> Option<TabInfo> {
    with_ui(|ui| {
        let tab = ui.tabs.get(&id)?;
        Some(tab.info(
            id,
            is_active(ui, id, tab.window),
            split_of(ui, id, tab.window),
        ))
    })
}

/// All tabs: window by window (oldest window first), each in strip order, then tabs of the
/// single layout by id. UI thread.
pub(crate) fn list_tabs() -> Vec<TabInfo> {
    with_ui(|ui| {
        let mut out = Vec::with_capacity(ui.tabs.len());
        for w in ui.windows.values() {
            for id in w.strip.tabs() {
                if let Some(tab) = ui.tabs.get(id) {
                    let split = split_of(ui, *id, tab.window);
                    out.push(tab.info(*id, w.strip.active() == Some(*id), split));
                }
            }
        }
        for (id, tab) in &ui.tabs {
            if tab.window.is_none_or(|key| !ui.windows.contains_key(&key)) {
                out.push(tab.info(*id, true, None));
            }
        }
        out
    })
}

/// Emit `tab.updated` with the tab's current state. UI thread.
pub(crate) fn emit_updated(id: TabId) {
    if let Some(info) = tab_info(id) {
        emit(topics::TAB_UPDATED, tab_events::updated(&info));
    }
}

pub(crate) fn emit_activated(id: TabId) {
    emit(topics::TAB_ACTIVATED, tab_events::activated(id));
}

// ------------------------------------------------------------------ run_on_ui

type Job = Box<dyn FnOnce() + Send>;

wrap_task! {
    struct UiJob {
        job: Arc<Mutex<Option<Job>>>,
    }

    impl Task {
        fn execute(&self) {
            let job = self.job.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(job) = job {
                job();
            }
        }
    }
}

/// Post `job` to the CEF UI thread (fire and forget). Callable from any thread.
/// Returns false if CEF no longer accepts tasks.
pub(crate) fn post_to_ui(job: Job) -> bool {
    let mut task = UiJob::new(Arc::new(Mutex::new(Some(job))));
    post_task(ThreadId::UI, Some(&mut task)) == 1
}

/// Post `job` to the CEF UI thread to run in `delay_ms` milliseconds. Any thread.
pub(crate) fn post_to_ui_delayed(job: Job, delay_ms: i64) -> bool {
    let mut task = UiJob::new(Arc::new(Mutex::new(Some(job))));
    post_delayed_task(ThreadId::UI, Some(&mut task), delay_ms) == 1
}

/// Run `f` on the CEF UI thread and await its result. Callable from any thread.
pub(crate) async fn run_on_ui<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, BackendError> {
    let (tx, rx) = oneshot::channel();
    let posted = post_to_ui(Box::new(move || {
        let _ = tx.send(f());
    }));
    if !posted {
        return Err(BackendError::ShuttingDown);
    }
    rx.await.map_err(|_| BackendError::ShuttingDown)
}

// ------------------------------------------------------------------ CDP plumbing

/// CDP transport over CEF's in-process DevTools channel (no remote-debugging port).
struct CefTransport {
    tab: TabId,
}

impl Transport for CefTransport {
    fn send(&self, message: String) -> BoxFuture<'static, Result<(), CdpError>> {
        let tab = self.tab;
        Box::pin(async move {
            let sent = run_on_ui(move || {
                let Some(browser) = with_ui(|ui| ui.tabs.get(&tab).map(|t| t.browser.clone()))
                else {
                    return false;
                };
                browser
                    .host()
                    .is_some_and(|host| host.send_dev_tools_message(Some(message.as_bytes())) == 1)
            })
            .await;
            match sent {
                Ok(true) => Ok(()),
                Ok(false) => Err(CdpError::Transport(format!("tab {tab} is gone"))),
                Err(_) => Err(CdpError::Closed),
            }
        })
    }
}

wrap_dev_tools_message_observer! {
    struct DevToolsObserver {
        cdp: Arc<CdpClient>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            if let Some(message) = message {
                self.cdp.handle_message(message);
            }
            1 // handled: CEF will not split it into method_result/event callbacks
        }
    }
}

// ------------------------------------------------------------------ tab lifecycle

/// Called from `LifeSpanHandler::on_after_created` for content browsers (UI thread).
/// `offscreen`: a windowless browser; its intent is `UiState::pending[pending_key]` (it has
/// no view id).
pub(crate) fn register_tab(browser: &Browser, offscreen: bool, pending_key: i32) {
    let id = TabId(browser.identifier());
    let cdp = Arc::new(CdpClient::new(CefTransport { tab: id }));
    let mut observer = DevToolsObserver::new(cdp.clone());
    let devtools = browser
        .host()
        .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));
    if devtools.is_none() {
        tracing::warn!(%id, "could not attach DevTools observer");
    }

    // Which intent created this browser? Found through the id of its BrowserView.
    let mut b = browser.clone();
    let view_id = browser_view_get_for_browser(Some(&mut b)).map_or(0, |v| v.id());
    let (load, _) = watch::channel(LoadState::default());

    // One short borrow: store the entry and place the tab in its window's strip.
    let (pending, index) = with_ui(|ui| {
        let key = if offscreen { pending_key } else { view_id };
        let mut pending = ui.pending.remove(&key);
        let window = pending.as_ref().and_then(|p| p.window);
        let url = pending.as_ref().map(|p| p.url.clone()).unwrap_or_default();
        let profile = pending
            .as_ref()
            .map(|p| p.profile.clone())
            .unwrap_or_default();
        let run = pending.as_ref().and_then(|p| p.run.clone());
        // A restored tab goes between its saved neighbours (tabs register in any order).
        let restore = pending.as_ref().and_then(|p| p.restore);
        let restore_index =
            restore.map(|(tag, index)| crate::session::registered(ui, tag, index, id));
        ui.tabs.insert(
            id,
            TabEntry {
                browser: browser.clone(),
                cdp,
                _devtools: devtools,
                title: String::new(),
                url,
                favicon: None,
                can_go_back: false,
                can_go_forward: false,
                window,
                load,
                profile,
                run,
                offscreen,
                zoom: 100.0,
            },
        );
        let wanted = restore_index.or_else(|| pending.as_ref().and_then(|p| p.index));
        let index = window
            .and_then(|key| ui.windows.get_mut(&key))
            .map(|w| w.strip.insert(id, wanted));
        let reply = pending.as_mut().and_then(|p| p.reply.take());
        let prepare = pending
            .as_mut()
            .and_then(|p| p.then_load.take().map(|url| (p.profile.name(), url)));
        (
            pending.map(|p| (p.window, p.activate, reply, prepare)),
            index,
        )
    });

    if let Some(info) = tab_info(id) {
        emit(
            topics::TAB_CREATED,
            tab_events::created(&info, index.unwrap_or(0)),
        );
    }
    tracing::debug!(%id, view_id, ?index, "tab registered");

    if offscreen && let Some(host) = browser.host() {
        // A windowless page has its own focus: it is always "the focused, visible page".
        host.was_hidden(0);
        host.set_focus(1);
    }

    if let Some((window, activate, reply, prepare)) = pending {
        if let (Some(key), true) = (window, activate) {
            crate::chrome::show_tab(key, id);
        }
        match prepare {
            // Identity first, then the real page; `tabs.open` answers after that.
            Some((profile, url)) => crate::hooks::prepare_then_load(id, profile, url, reply),
            None => {
                if let Some(reply) = reply {
                    let _ = reply.send(id);
                }
            }
        }
    }
}

/// Called from `LifeSpanHandler::on_before_close` for content browsers (UI thread).
pub(crate) fn unregister_tab(browser: &Browser) {
    let id = TabId(browser.identifier());
    // Its split view ends (usually done in `chrome::detach_tab` already).
    let window = with_ui(|ui| ui.tabs.get(&id).and_then(|t| t.window));
    if let Some(key) = window {
        crate::chrome::end_split_with(key, id);
    }
    crate::prompts::drop_for_tab(id);
    let (removed, next) = with_ui(|ui| {
        let removed = ui.tabs.remove(&id);
        // Usually already gone from the strip (see `chrome::detach_tab`); otherwise the
        // tab died on its own (renderer gone, window torn down).
        let next = removed.as_ref().and_then(|t| t.window).and_then(|key| {
            let w = ui.windows.get_mut(&key)?;
            let removed = w.strip.remove(id)?;
            (!w.closing).then_some((key, removed.new_active?))
        });
        (removed, next)
    });
    // Dropped outside the borrow: releasing the registration calls into CEF.
    if let Some(tab) = removed {
        tab.cdp.close();
        let profile = tab.profile.clone();
        drop(tab);
        emit(topics::TAB_CLOSED, tab_events::closed(id));
        // An in-memory profile goes away with its last tab (later, outside CEF's callback).
        post_to_ui(Box::new(move || {
            crate::profiles::release_if_unused(&profile)
        }));
    }
    if let Some((key, next)) = next {
        // Not from inside CEF's close callback: show the neighbour a moment later.
        post_to_ui(Box::new(move || crate::chrome::show_tab(key, next)));
    }
    tracing::debug!(%id, "tab unregistered");
}

/// Remember one of our own UI/palette browsers (not a tab). UI thread.
pub(crate) fn register_chrome_view(browser: &Browser, kind: crate::chrome::ViewKind) {
    let view = crate::chrome::ChromeView {
        kind,
        browser: browser.clone(),
    };
    with_ui(|ui| ui.chrome_views.insert(browser.identifier(), view));
    tracing::debug!(id = browser.identifier(), ?kind, "chrome view registered");
}

pub(crate) fn unregister_chrome_view(browser: &Browser) {
    let removed = with_ui(|ui| ui.chrome_views.remove(&browser.identifier()));
    drop(removed); // released outside the UiState borrow
}

/// Change a tab's entry, then emit `tab.updated` if `update` returned true. UI thread.
pub(crate) fn update_tab(browser: &Browser, update: impl FnOnce(&mut TabEntry) -> bool) {
    let id = TabId(browser.identifier());
    let changed = with_ui(|ui| ui.tabs.get_mut(&id).is_some_and(update));
    if changed {
        emit_updated(id);
    }
}

/// Drop every CEF object we still hold before `cef::shutdown()`; leftover references
/// make shutdown wait forever. UI thread only.
pub(crate) fn clear_ui_state() {
    let state = with_ui(std::mem::take);
    drop(state);
}
