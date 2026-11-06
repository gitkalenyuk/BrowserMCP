//! Profiles in CEF: one `CefRequestContext` per profile.
//!
//! * The profile the browser started with (`--profile`, normally `default`) uses the global
//!   request context (cache = `data/profiles/<id>`, set in `Settings`).
//! * Any other profile gets its own context, created on first use, with its cache in
//!   `data/profiles/<id>` (must be inside `root_cache_path` = `data/profiles`).
//! * Incognito: a context without a cache path (in memory). `profile + incognito` = the
//!   in-memory profile of that name (rows of a mass run get one each); incognito without a
//!   profile = the shared `incognito` profile. In-memory contexts are dropped when their last
//!   tab closes.
//!
//! Settings from `profile.create` ([`ProfileSettings`]) are applied with `SetPreference`
//! (proxy, WebRTC, Accept-Language; see `bmcp_profiles::prefs`) — right away when the context
//! exists, otherwise in `on_request_context_initialized`. Proxy passwords never reach
//! Chromium: an authenticated upstream is always behind the local forwarder (live finding:
//! `GetAuthCredentials` was not called for proxy challenges, Chromium's own login prompt took
//! over and crashed without a UI).
//!
//! UI thread only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use bmcp_core::{BackendError, ProfileId};
use bmcp_profiles::{Prefs, ProfileSettings};
use cef::*;

use crate::bridge::with_ui;

/// Which request context a tab uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ProfileKey {
    /// `None` = the profile the browser started with.
    pub id: Option<String>,
    /// In memory only.
    pub incognito: bool,
}

/// Name of the shared in-memory profile (`incognito: true` without a profile).
const INCOGNITO: &str = "incognito";

impl ProfileKey {
    /// From `tabs.open {profile, incognito}`. The launch profile on disk becomes `id: None`.
    pub fn from_options(profile: Option<&str>, incognito: bool) -> Result<Self, BackendError> {
        let launch = launch().profile.as_str();
        let id = match profile {
            Some(p) => {
                ProfileId::new(p).map_err(|_| {
                    BackendError::Invalid(format!(
                        "profile {p:?} is invalid: use 1-64 chars of a-z, 0-9, '-' or '_'"
                    ))
                })?;
                Some(p.to_string())
            }
            None if incognito => Some(INCOGNITO.to_string()),
            None => None,
        };
        let id = id.filter(|id| incognito || id != launch);
        Ok(ProfileKey { id, incognito })
    }

    /// The profile's name (`default`, `work`, …) as the rest of the app knows it.
    pub fn name(&self) -> String {
        self.id.clone().unwrap_or_else(|| launch().profile.clone())
    }

    /// The global context (the launch profile on disk)?
    pub fn is_global(&self) -> bool {
        self.id.is_none()
    }
}

/// Facts from startup.
pub(crate) struct Launch {
    pub data_root: PathBuf,
    pub profile: String,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

pub(crate) fn set_launch(data_root: PathBuf, profile: String) {
    let _ = LAUNCH.set(Launch { data_root, profile });
}

pub(crate) fn launch() -> &'static Launch {
    LAUNCH.get_or_init(|| Launch {
        data_root: PathBuf::from("data"),
        profile: ProfileId::DEFAULT.to_string(),
    })
}

/// Per-profile CEF state (inside `UiState`).
#[derive(Default)]
pub(crate) struct ProfilesState {
    /// Live contexts of non-global profiles.
    pub contexts: BTreeMap<ProfileKey, RequestContext>,
    /// Settings by profile name (applied to every context of that name).
    pub settings: BTreeMap<String, ProfileSettings>,
    /// Contexts whose `on_request_context_initialized` has run.
    pub ready: BTreeSet<ProfileKey>,
    /// Work waiting for a context to become ready (browser creation, see [`when_ready`]).
    pub waiting: BTreeMap<ProfileKey, Vec<ReadyJob>>,
}

/// Something to do once a profile's request context is initialized.
pub(crate) type ReadyJob = Box<dyn FnOnce() + Send>;

/// Run `job` on the UI thread once `key`'s request context is initialized (now, if it is).
///
/// Pitfall: creating a browser (BrowserView or windowless) with a context that is still
/// initializing fails (`browser_view_create` returns None, `CreateBrowserSync` too), so every
/// tab of a fresh profile waits for `on_request_context_initialized`. UI thread.
pub(crate) fn when_ready(key: &ProfileKey, job: ReadyJob) {
    if key.is_global() {
        job();
        return;
    }
    if context_for(key).is_none() {
        tracing::error!(profile = key.name(), "request context could not be created");
        return; // dropping the job drops its reply: the caller sees an error
    }
    let mut job = Some(job);
    let now = with_ui(|ui| {
        if ui.profiles.ready.contains(key) {
            job.take()
        } else {
            let queue = ui.profiles.waiting.entry(key.clone()).or_default();
            queue.extend(job.take());
            None
        }
    });
    if let Some(job) = now {
        job();
    }
}

/// The context of `key` finished initializing: run what waited for it (a moment later, not
/// inside CEF's callback). UI thread.
fn mark_ready(key: ProfileKey) {
    let jobs = with_ui(|ui| {
        ui.profiles.ready.insert(key.clone());
        ui.profiles.waiting.remove(&key).unwrap_or_default()
    });
    for job in jobs {
        crate::bridge::post_to_ui(job);
    }
}

/// The request context for `key`, created on first use. `None` = use the global context
/// (pass `None` to CEF). UI thread.
pub(crate) fn context_for(key: &ProfileKey) -> Option<RequestContext> {
    if key.is_global() {
        return None;
    }
    if let Some(ctx) = with_ui(|ui| ui.profiles.contexts.get(key).cloned()) {
        return Some(ctx);
    }
    let name = key.name();
    let cache = if key.incognito {
        String::new() // in memory
    } else {
        let dir = launch().data_root.join("profiles").join(&name);
        let _ = std::fs::create_dir_all(&dir);
        crate::native_path(&dir)
    };
    // Accept-Language from the profile's settings already at creation (the pref is set again
    // in `on_request_context_initialized`, and on later changes).
    let language = with_ui(|ui| {
        ui.profiles
            .settings
            .get(&name)
            .and_then(|s| s.prefs.accept_language.clone())
    })
    .map(|l| bmcp_profiles::prefs::strip_q_values(&l))
    .unwrap_or_default();
    let settings = RequestContextSettings {
        cache_path: CefString::from(cache.as_str()),
        persist_session_cookies: i32::from(!key.incognito),
        accept_language_list: CefString::from(language.as_str()),
        ..Default::default()
    };
    let mut handler = ProfileContextHandler::new(name.clone(), key.clone());
    let context = request_context_create_context(Some(&settings), Some(&mut handler))?;
    // `bmcp://` (the new tab page) is registered on the global context only; every other
    // context needs its own registration.
    crate::scheme::register_on(&context);
    with_ui(|ui| ui.profiles.contexts.insert(key.clone(), context.clone()));
    tracing::debug!(
        profile = name,
        incognito = key.incognito,
        "request context created"
    );
    Some(context)
}

/// The context to use for cookies etc. of `key`: its own, or the global one.
pub(crate) fn context_or_global(key: &ProfileKey) -> Option<RequestContext> {
    context_for(key).or_else(request_context_get_global_context)
}

/// Drop an in-memory profile's context once its last tab closed. UI thread.
pub(crate) fn release_if_unused(key: &ProfileKey) {
    if !key.incognito {
        return; // on-disk profiles stay loaded (cheap, and keep their cookies flushed)
    }
    let removed = with_ui(|ui| {
        let in_use = ui.tabs.values().any(|t| t.profile == *key);
        if in_use {
            None
        } else {
            ui.profiles.ready.remove(key);
            ui.profiles.contexts.remove(key)
        }
    });
    if removed.is_some() {
        tracing::debug!(profile = key.name(), "in-memory profile released");
    }
    drop(removed); // released outside the UiState borrow
}

/// `ProfileBackend::configure`: remember and apply. UI thread.
pub(crate) fn configure(settings: ProfileSettings) {
    let name = settings.id.clone();
    let prefs = settings.prefs.clone();
    with_ui(|ui| ui.profiles.settings.insert(name.clone(), settings));
    // Every live context of that profile (on disk and in memory), or the global one.
    let mut contexts: Vec<RequestContext> = with_ui(|ui| {
        ui.profiles
            .contexts
            .iter()
            .filter(|(k, _)| k.name() == name)
            .map(|(_, c)| c.clone())
            .collect()
    });
    if name == launch().profile
        && let Some(global) = request_context_get_global_context()
    {
        contexts.push(global);
    }
    for context in contexts {
        apply_prefs(&context, &prefs);
    }
}

/// `ProfileBackend::forget`: refuse while tabs are open, else drop the context(s).
pub(crate) fn forget(name: &str) -> Result<(), BackendError> {
    let busy = with_ui(|ui| {
        ui.tabs
            .values()
            .filter(|t| t.profile.name() == name)
            .count()
    });
    if busy > 0 {
        return Err(BackendError::Invalid(format!(
            "profile {name:?} still has {busy} open tab(s)"
        )));
    }
    let removed: Vec<RequestContext> = with_ui(|ui| {
        ui.profiles.settings.remove(name);
        let keys: Vec<ProfileKey> = ui
            .profiles
            .contexts
            .keys()
            .filter(|k| k.name() == name)
            .cloned()
            .collect();
        keys.iter()
            .filter_map(|k| {
                ui.profiles.ready.remove(k);
                ui.profiles.contexts.remove(k)
            })
            .collect()
    });
    drop(removed);
    Ok(())
}

/// Set every preference of `prefs` on `context`. UI thread.
pub(crate) fn apply_prefs(context: &RequestContext, prefs: &Prefs) {
    for (name, value) in prefs.to_preferences() {
        let Some(mut value) = json_to_cef(&value) else {
            continue;
        };
        let cef_name = CefString::from(name);
        if context.can_set_preference(Some(&cef_name)) != 1 {
            // Not registered in this CEF build (e.g. the webrtc.* booleans in Alloy).
            tracing::debug!(name, "preference cannot be set");
            continue;
        }
        // Pitfall: the error out-parameter must point at a real string. `CefString::default()`
        // is passed as NULL, and CEF's C wrapper then returns 0 without doing anything (every
        // SetPreference "failed" silently). A non-empty string has storage CEF can write to.
        let mut error = CefString::from(" ");
        if context.set_preference(Some(&cef_name), Some(&mut value), Some(&mut error)) != 1 {
            tracing::warn!(name, error = %error, "preference rejected");
        } else {
            tracing::debug!(name, "preference set");
        }
    }
}

/// JSON → `CefValue` (via CEF's own JSON parser: objects, lists, strings, bools, numbers).
pub(crate) fn json_to_cef(value: &serde_json::Value) -> Option<Value> {
    let text = value.to_string();
    // The parser's result is read-only; hand CEF a writable copy.
    parse_json(
        Some(&CefString::from(text.as_str())),
        JsonParserOptions::default(),
    )?
    .copy()
}

wrap_request_context_handler! {
    struct ProfileContextHandler {
        profile: String,
        key: ProfileKey,
    }

    impl RequestContextHandler {
        /// The context is ready: apply the profile's stored settings.
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let Some(context) = request_context else { return };
            let prefs = with_ui(|ui| ui.profiles.settings.get(&self.profile).map(|s| s.prefs.clone()));
            if let Some(prefs) = prefs {
                apply_prefs(context, &prefs);
            }
            // Tabs waiting for this context can be created now.
            mark_ready(self.key.clone());
        }

        /// Requests without a browser (service workers, prefetch) of this profile go through
        /// the adblock gate too.
        fn resource_request_handler(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            if browser.is_some() {
                return None; // the browser's own RequestHandler decides
            }
            crate::net::request_handler()
        }
    }
}
