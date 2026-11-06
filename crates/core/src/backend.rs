//! The boundary between "what the browser can do" and "how CEF does it".
//!
//! API servers, scripts and agents only ever see `Arc<dyn BrowserBackend>`; the CEF crate
//! implements it. That keeps every API surface testable with a fake backend.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Stable id of a tab for the lifetime of the process (CEF browser identifier).
/// On the wire it is a plain number: `3`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct TabId(pub i32);

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// URL of the browser's own new tab page (served by the CEF crate from `bmcp://ui/`).
pub const NEW_TAB_URL: &str = "bmcp://ui/newtab.html";

/// One tab as every API surface sees it. On the wire the fields are camelCase
/// (`canGoBack`), which is what the Svelte UI (`ui/src/lib/api/types.ts`) expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    /// Tab id, used as `tab` argument by other commands.
    pub id: TabId,
    /// Page title.
    pub title: String,
    /// Current URL.
    pub url: String,
    /// True while the page is still loading.
    pub loading: bool,
    /// Favicon URL; null until the page reports one.
    #[serde(default)]
    pub favicon: Option<String>,
    /// True when `nav.back` would do something.
    #[serde(default)]
    pub can_go_back: bool,
    /// True when `nav.forward` would do something.
    #[serde(default)]
    pub can_go_forward: bool,
    /// True for the tab shown in its window (one per window).
    #[serde(default)]
    pub active: bool,
    /// Tab group id. Always null for now (tab groups are not implemented yet).
    #[serde(default)]
    pub group_id: Option<String>,
    /// Agent holding the tab lock. Always null for now (agent locks come later).
    #[serde(default)]
    pub locked_by: Option<String>,
}

impl TabInfo {
    /// A tab with just an id and a URL (title = URL), everything else empty/false.
    pub fn new(id: TabId, url: impl Into<String>) -> Self {
        let url = url.into();
        TabInfo {
            id,
            title: url.clone(),
            url,
            loading: false,
            favicon: None,
            can_go_back: false,
            can_go_forward: false,
            active: false,
            group_id: None,
            locked_by: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BackendError {
    #[error("tab {0} not found")]
    TabNotFound(TabId),
    #[error("no tab is open")]
    NoTabs,
    #[error("timed out after {0:?}")]
    Timeout(Duration),
    #[error("browser is shutting down")]
    ShuttingDown,
    #[error("CDP: {0}")]
    Cdp(String),
}

pub type BackendResult<T> = Result<T, BackendError>;

/// One Chrome DevTools Protocol event (`{"method": ..., "params": ...}`) from a tab, optionally
/// from a flattened child session (iframe, worker).
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

/// Receiver of a tab's CDP events. Lagging receivers get `RecvError::Lagged(n)`.
pub type CdpEvents = tokio::sync::broadcast::Receiver<CdpEvent>;

pub trait BrowserBackend: Send + Sync + 'static {
    /// All open tabs, window by window, each window in tab-strip order.
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>>;

    /// Open a new tab in the current window and return its id once the browser exists.
    /// `background = false` also makes it the active tab.
    fn open_tab(&self, url: String, background: bool) -> BoxFuture<'_, BackendResult<TabId>>;

    /// Close a tab (its page may still run `beforeunload`). Closing the last tab of a
    /// window closes the window; closing the last window quits.
    fn close_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>>;

    /// Show this tab in its window (and hide the one that was shown).
    fn activate_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>>;

    /// Stop loading the page.
    fn stop_loading(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>>;

    /// Navigate and wait until the page finished loading (or `timeout`).
    fn navigate(
        &self,
        tab: TabId,
        url: String,
        timeout: Duration,
    ) -> BoxFuture<'_, BackendResult<TabInfo>>;

    /// Raw Chrome DevTools Protocol call on the tab's page target.
    fn cdp(&self, tab: TabId, method: String, params: Value)
    -> BoxFuture<'_, BackendResult<Value>>;

    /// Raw CDP call inside a flattened child session of the tab: an out-of-process iframe
    /// attached with `Target.setAutoAttach {flatten: true}` (its `sessionId` comes with the
    /// `Target.attachedToTarget` event). `None` is the page itself, like [`Self::cdp`].
    /// Backends without child sessions refuse `Some(..)`.
    fn cdp_session(
        &self,
        tab: TabId,
        session_id: Option<String>,
        method: String,
        params: Value,
    ) -> BoxFuture<'_, BackendResult<Value>> {
        match session_id {
            None => self.cdp(tab, method, params),
            Some(_) => Box::pin(async {
                Err(BackendError::Cdp(
                    "this backend has no CDP child sessions".into(),
                ))
            }),
        }
    }

    /// Subscribe to the tab's CDP events from now on (e.g. after `Network.enable`,
    /// `Network.requestWillBeSent` arrives here). Enable the CDP domain with [`Self::cdp`].
    /// The receiver ends (`RecvError::Closed`) when the tab closes.
    fn cdp_events(&self, tab: TabId) -> BoxFuture<'_, BackendResult<CdpEvents>> {
        let _ = tab;
        Box::pin(async { Err(BackendError::Cdp("this backend has no CDP events".into())) })
    }

    /// Close every window and leave the message loop.
    fn quit(&self) -> BoxFuture<'_, BackendResult<()>>;
}
