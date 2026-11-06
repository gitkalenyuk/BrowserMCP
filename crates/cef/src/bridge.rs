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

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use bmcp_cdp::{BoxFuture, CdpClient, CdpError, Transport};
use bmcp_core::{BackendError, TabId, TabInfo};
use cef::*;
use tokio::sync::{broadcast, oneshot, watch};

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
    pub load: watch::Sender<LoadState>,
}

impl TabEntry {
    pub fn info(&self, id: TabId) -> TabInfo {
        TabInfo {
            id,
            title: self.title.clone(),
            url: self.url.clone(),
            loading: self.load.borrow().loading,
        }
    }
}

/// Everything the shell needs to open another window later.
#[derive(Clone)]
pub(crate) struct ShellContext {
    pub style: RuntimeStyle,
    pub browsers: Arc<crate::shell::Browsers>,
}

#[derive(Default)]
pub(crate) struct UiState {
    pub tabs: BTreeMap<TabId, TabEntry>,
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

/// Fires once per created tab; lets `open_tab` learn the id of the browser it created.
pub(crate) fn tab_created() -> &'static broadcast::Sender<TabId> {
    static CREATED: OnceLock<broadcast::Sender<TabId>> = OnceLock::new();
    CREATED.get_or_init(|| broadcast::channel(64).0)
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

/// Post `job` to the CEF UI thread. Returns false if CEF no longer accepts tasks.
fn post_to_ui(job: Job) -> bool {
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

/// Called from `LifeSpanHandler::on_after_created` (UI thread).
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
    let (load, _) = watch::channel(LoadState::default());
    with_ui(|ui| {
        ui.tabs.insert(
            id,
            TabEntry {
                browser: browser.clone(),
                cdp,
                _devtools: devtools,
                title: String::new(),
                url: String::new(),
                load,
            },
        )
    });
    let _ = tab_created().send(id);
    tracing::debug!(%id, "tab registered");
}

/// Called from `LifeSpanHandler::on_before_close` (UI thread).
pub(crate) fn unregister_tab(browser: &Browser) {
    let id = TabId(browser.identifier());
    let removed = with_ui(|ui| ui.tabs.remove(&id));
    // Dropped outside the borrow: releasing the registration calls into CEF.
    if let Some(tab) = removed {
        tab.cdp.close();
    }
    tracing::debug!(%id, "tab unregistered");
}

pub(crate) fn update_tab(browser: &Browser, update: impl FnOnce(&mut TabEntry)) {
    let id = TabId(browser.identifier());
    with_ui(|ui| {
        if let Some(tab) = ui.tabs.get_mut(&id) {
            update(tab);
        }
    });
}

/// Drop every CEF object we still hold before `cef::shutdown()`; leftover references
/// make shutdown wait forever. UI thread only.
pub(crate) fn clear_ui_state() {
    let state = with_ui(std::mem::take);
    drop(state);
}
