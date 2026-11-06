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

/// Which half of a split view a tab is shown in (`tabs.split`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SplitSide {
    Left,
    Right,
}

/// A tab that is one half of a split view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TabSplit {
    /// The half this tab is shown in.
    pub side: SplitSide,
    /// The tab in the other half.
    pub other: TabId,
    /// Width of the left half as a fraction of the page area (0.15–0.85).
    pub ratio: f64,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// One tab as every API surface sees it. On the wire the fields are camelCase
/// (`canGoBack`), which is what the Svelte UI (`ui/src/lib/api/types.ts`) expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Profile (= workspace in the UI) the tab runs in. Absent for backends without
    /// profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// In-memory profile (nothing written to disk). Absent = false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub incognito: bool,
    /// Off-screen (windowless automation) tab: not shown in any window's tab strip.
    /// Absent = false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub offscreen: bool,
    /// Browser window that shows the tab. Absent for off-screen tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<i32>,
    /// Set while the tab is one half of a split view (`tabs.split`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<TabSplit>,
    /// Page zoom in percent when it is not 100 (`page.zoom`, Ctrl+= / Ctrl+-).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<u16>,
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
            profile: None,
            incognito: false,
            offscreen: false,
            window: None,
            split: None,
            zoom: None,
        }
    }
}

/// How `page.zoom` changes a tab's zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomChange {
    /// Set this zoom, in percent (100 = normal).
    Percent(f64),
    /// Go this many preset steps up (+) or down (-); 0 = back to 100 %.
    Steps(i32),
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
    /// This backend cannot do that (e.g. profiles in the test backend).
    #[error("not supported: {0}")]
    Unsupported(String),
    /// The request itself is wrong (unknown profile, bad combination of options).
    #[error("{0}")]
    Invalid(String),
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

/// Where [`BrowserBackend::open_tab_with`] puts a new tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowTarget {
    /// A tab in the window used last (the normal case).
    #[default]
    Current,
    /// A new browser window of its own (visible, laid out, with the browser UI).
    New,
    /// No window at all: an off-screen (windowless) page with its own size and focus.
    /// For automation: parallel runs, typing into several pages at once.
    Offscreen,
}

/// Everything about a new tab besides its URL. `Default` = the classic `open_tab`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenTabOptions {
    /// Open without switching to it.
    pub background: bool,
    /// Profile (cookies, cache, proxy, identity). `None` = the profile the browser started with.
    pub profile: Option<String>,
    /// Keep everything in memory (nothing written to disk). With `profile`: the in-memory
    /// profile of that name; without: the shared incognito profile.
    pub incognito: bool,
    pub window: WindowTarget,
    /// Mass run this tab belongs to (its downloads go to `data/runs/<run>/downloads`).
    pub run: Option<String>,
}

impl OpenTabOptions {
    /// True when only `background` is set (what plain `open_tab` can do).
    pub fn is_plain(&self) -> bool {
        self.profile.is_none()
            && !self.incognito
            && self.window == WindowTarget::Current
            && self.run.is_none()
    }
}

pub trait BrowserBackend: Send + Sync + 'static {
    /// All open tabs, window by window, each window in tab-strip order.
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>>;

    /// Open a new tab in the current window and return its id once the browser exists.
    /// `background = false` also makes it the active tab.
    fn open_tab(&self, url: String, background: bool) -> BoxFuture<'_, BackendResult<TabId>>;

    /// Open a tab with options (profile, incognito, own window). The default implementation
    /// only handles plain options; the CEF backend does all of them.
    fn open_tab_with(
        &self,
        url: String,
        options: OpenTabOptions,
    ) -> BoxFuture<'_, BackendResult<TabId>> {
        if options.is_plain() {
            return self.open_tab(url, options.background);
        }
        Box::pin(async {
            Err(BackendError::Unsupported(
                "profiles and own windows need the real browser".into(),
            ))
        })
    }

    /// Close a tab (its page may still run `beforeunload`). Closing the last tab of a
    /// window closes the window; closing the last window quits.
    fn close_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>>;

    /// Close a tab, but never its window: when it is the last tab of its window, the new tab
    /// page takes its place first. Used for automation callers, so one agent closing "its"
    /// tab cannot close the window (or quit the browser) others are using. Default: plain
    /// [`Self::close_tab`].
    fn close_tab_keep_window(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        self.close_tab(tab)
    }

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

    /// Show two tabs of the same window side by side (`left` | `right`); `ratio` = width
    /// of the left half (0.15–0.85, default 0.5). Default: unsupported.
    fn split_tabs(
        &self,
        left: TabId,
        right: TabId,
        ratio: Option<f64>,
    ) -> BoxFuture<'_, BackendResult<()>> {
        let _ = (left, right, ratio);
        Box::pin(async { Err(BackendError::Unsupported("split view".into())) })
    }

    /// End the split view `tab` is part of (`None`: the one of the window used last).
    /// Returns the tabs that were split. Default: unsupported.
    fn unsplit(&self, tab: Option<TabId>) -> BoxFuture<'_, BackendResult<Vec<TabId>>> {
        let _ = tab;
        Box::pin(async { Err(BackendError::Unsupported("split view".into())) })
    }

    /// Re-open the most recently closed tab (Ctrl+Shift+T). `None` = nothing to restore.
    /// Default: unsupported.
    fn restore_closed(&self) -> BoxFuture<'_, BackendResult<Option<TabId>>> {
        Box::pin(async { Err(BackendError::Unsupported("closed tabs".into())) })
    }

    /// Change a tab's page zoom; returns the new zoom in percent. Default: unsupported.
    fn zoom(&self, tab: TabId, change: ZoomChange) -> BoxFuture<'_, BackendResult<f64>> {
        let _ = (tab, change);
        Box::pin(async { Err(BackendError::Unsupported("page zoom".into())) })
    }

    /// Close every window and leave the message loop.
    fn quit(&self) -> BoxFuture<'_, BackendResult<()>>;
}
