//! What the profile commands need from the browser. Implemented by the CEF crate
//! (`bmcp_cef::CefBackend`); the app hands it to [`crate::ProfilesService::set_backend`].

use bmcp_core::{BackendResult, BoxFuture, TabId};
use schemars::JsonSchema;
use serde::Serialize;

use crate::cookies::CookieInfo;
use crate::prefs::Prefs;

/// Username/password for an HTTP proxy that is used directly (without our forwarder).
/// Answered from CEF's `GetAuthCredentials` when the proxy asks.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxyAuth {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProxyAuth({}:***)", self.username)
    }
}

/// Everything the browser applies to one profile's request context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSettings {
    pub id: String,
    pub prefs: Prefs,
    pub proxy_auth: Option<ProxyAuth>,
}

/// A profile the browser has a live request context for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct OpenProfile {
    pub id: String,
    /// In memory only (nothing on disk).
    pub incognito: bool,
    /// Its open tabs.
    pub tabs: Vec<TabId>,
}

/// Whose cookies: a profile (default: the one the browser started with), on disk or the
/// in-memory one of that name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CookieTarget {
    pub profile: Option<String>,
    pub incognito: bool,
}

pub trait ProfileBackend: Send + Sync + 'static {
    /// Remember these settings for the profile and apply them to its request context (now,
    /// if it exists; otherwise when its first tab opens).
    fn configure(&self, settings: ProfileSettings) -> BoxFuture<'_, BackendResult<()>>;

    /// Forget a profile (fails with `Invalid` while it still has open tabs).
    fn forget(&self, id: String) -> BoxFuture<'_, BackendResult<()>>;

    /// Profiles with a live request context.
    fn open_profiles(&self) -> BoxFuture<'_, BackendResult<Vec<OpenProfile>>>;

    /// Cookies of a profile, optionally only those of `domain` (and its subdomains).
    fn cookies(
        &self,
        target: CookieTarget,
        domain: Option<String>,
    ) -> BoxFuture<'_, BackendResult<Vec<CookieInfo>>>;

    /// Set cookies; one `true`/`false` per cookie (accepted by the cookie store?).
    fn set_cookies(
        &self,
        target: CookieTarget,
        cookies: Vec<CookieInfo>,
    ) -> BoxFuture<'_, BackendResult<Vec<bool>>>;

    /// Delete cookies of `domain` (all when `None`), optionally only those named `name`.
    /// Returns how many were deleted.
    fn delete_cookies(
        &self,
        target: CookieTarget,
        domain: Option<String>,
        name: Option<String>,
    ) -> BoxFuture<'_, BackendResult<usize>>;
}
