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
use bmcp_core::{BackendError, EventBus, TabId, TabInfo};
use cef::*;
use serde_json::Value;
use tokio::sync::{oneshot, watch};

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
    /// Key of the composed window that shows this tab (None in the single layout).
    pub window: Option<i32>,
    pub load: watch::Sender<LoadState>,
}

impl TabEntry {
    pub fn info(&self, id: TabId, active: bool) -> TabInfo {
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

fn emit(topic: &str, data: Value) {
    if let Some(bus) = EVENTS.get() {
        bus.emit(topic, data);
    }
}

/// Is `id` the shown tab of its window? (Single-layout tabs are always "active".)
fn is_active(ui: &UiState, id: TabId, window: Option<i32>) -> bool {
    match window.and_then(|key| ui.windows.get(&key)) {
        Some(w) => w.strip.active() == Some(id),
        None => true,
    }
}

/// Current info of one tab. UI thread.
pub(crate) fn tab_info(id: TabId) -> Option<TabInfo> {
    with_ui(|ui| {
        let tab = ui.tabs.get(&id)?;
        Some(tab.info(id, is_active(ui, id, tab.window)))
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
                    out.push(tab.info(*id, w.strip.active() == Some(*id)));
                }
            }
        }
        for (id, tab) in &ui.tabs {
            if tab.window.is_none_or(|key| !ui.windows.contains_key(&key)) {
                out.push(tab.info(*id, true));
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
pub(crate) fn register_tab(browser: &Browser) {
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
        let mut pending = ui.pending.remove(&view_id);
        let window = pending.as_ref().and_then(|p| p.window);
        let url = pending.as_ref().map(|p| p.url.clone()).unwrap_or_default();
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
            },
        );
        let index = window
            .and_then(|key| ui.windows.get_mut(&key))
            .map(|w| w.strip.insert(id, pending.as_ref().and_then(|p| p.index)));
        let reply = pending.as_mut().and_then(|p| p.reply.take());
        (pending.map(|p| (p.window, p.activate, reply)), index)
    });

    if let Some(info) = tab_info(id) {
        emit(
            topics::TAB_CREATED,
            tab_events::created(&info, index.unwrap_or(0)),
        );
    }
    tracing::debug!(%id, view_id, ?index, "tab registered");

    if let Some((window, activate, reply)) = pending {
        if let (Some(key), true) = (window, activate) {
            crate::chrome::show_tab(key, id);
        }
        if let Some(reply) = reply {
            let _ = reply.send(id);
        }
    }
}

/// Called from `LifeSpanHandler::on_before_close` for content browsers (UI thread).
pub(crate) fn unregister_tab(browser: &Browser) {
    let id = TabId(browser.identifier());
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
        drop(tab);
        emit(topics::TAB_CLOSED, tab_events::closed(id));
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
