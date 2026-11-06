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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabInfo {
    /// Tab id, used as `tab` argument by other commands.
    pub id: TabId,
    /// Page title.
    pub title: String,
    /// Current URL.
    pub url: String,
    /// True while the page is still loading.
    pub loading: bool,
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
    /// All open tabs, oldest first.
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>>;

    /// Open a new tab and return its id once the browser exists.
    fn open_tab(&self, url: String) -> BoxFuture<'_, BackendResult<TabId>>;

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
