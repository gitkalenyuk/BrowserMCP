//! [`LanguagesService`]: the stored settings plus the browser that applies them, kept in the
//! registry's extensions.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use bmcp_core::{BackendResult, BoxFuture, CmdError, Ctx, DataDir, ErrorCode};
use schemars::JsonSchema;
use serde::Serialize;

use crate::config::{LanguageSettings, available_locales, default_locales_dir, system_language};

/// A profile whose request context keeps its own Accept-Language (see
/// [`crate::accept_language_for`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ProfileLanguage {
    pub profile: String,
    /// Its list, e.g. `de-DE,de,en-US,en`.
    pub accept_language: String,
}

/// What the browser reports about the languages it applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct BackendLanguages {
    /// Can the spellcheck dictionaries be set in this Chromium build (`spellcheck.dictionaries`
    /// is a registered preference)? `None` = not known (no browser).
    pub spellcheck_supported: Option<bool>,
    /// The dictionaries Chromium uses now (`[]` = spellcheck off); `None` = not known.
    pub spellcheck_current: Option<Vec<String>>,
    /// Profiles that keep their own language instead of the global list.
    pub own_language: Vec<ProfileLanguage>,
}

/// What the language commands need from the browser. Implemented by `bmcp_cef::CefBackend`;
/// the app hands it to [`LanguagesService::set_backend`].
pub trait LanguageBackend: Send + Sync + 'static {
    /// Use these website languages (`intl.accept_languages`) on every request context whose
    /// profile has no language of its own, now and for contexts created later, and these
    /// spellcheck dictionaries on every context (`None` = leave Chromium's own setting).
    fn apply_languages(
        &self,
        web: Vec<String>,
        spellcheck: Option<Vec<String>>,
    ) -> BoxFuture<'_, BackendResult<BackendLanguages>>;

    /// The same report without changing anything.
    fn languages(&self) -> BoxFuture<'_, BackendResult<BackendLanguages>>;
}

/// The stored settings and the browser that applies them.
pub struct LanguagesService {
    config_dir: PathBuf,
    locales_dir: PathBuf,
    settings: Mutex<LanguageSettings>,
    /// `browser_locale` as it was when the browser started (Chromium reads it only then).
    started_locale: Option<String>,
    backend: RwLock<Option<Arc<dyn LanguageBackend>>>,
    /// Held by `browser.languages_set` from reading the settings to the browser applying
    /// them, so concurrent calls (the UI and an agent) neither lose an update nor reach CEF
    /// out of order. Async: it is held across the browser call.
    writes: tokio::sync::Mutex<()>,
    /// The last `apply_languages` failed after the settings were stored: the browser may not
    /// match them, so the next set applies again even when it changes nothing.
    out_of_sync: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LanguagesService {
    /// Settings from `config_dir/languages.json`; Chromium locales from `locales_dir`.
    pub fn new(config_dir: impl Into<PathBuf>, locales_dir: impl Into<PathBuf>) -> Self {
        let config_dir = config_dir.into();
        let settings = LanguageSettings::load(&config_dir);
        LanguagesService {
            started_locale: settings.browser_locale.clone(),
            config_dir,
            locales_dir: locales_dir.into(),
            settings: Mutex::new(settings),
            backend: RwLock::new(None),
            writes: tokio::sync::Mutex::new(()),
            out_of_sync: AtomicBool::new(false),
        }
    }

    /// One change at a time (see the `writes` field); hold the guard until the browser applied.
    pub async fn write_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.writes.lock().await
    }

    /// Did the last attempt to apply the stored settings to the browser fail?
    pub fn out_of_sync(&self) -> bool {
        self.out_of_sync.load(Ordering::SeqCst)
    }

    pub fn set_out_of_sync(&self, failed: bool) {
        self.out_of_sync.store(failed, Ordering::SeqCst);
    }

    /// The service of the registry; created on first use from the data folder when the app
    /// did not [`crate::install`] one (tests, the schema dump).
    pub fn from_ctx(ctx: &Ctx) -> Result<Arc<LanguagesService>, CmdError> {
        if let Some(service) = ctx.extensions.get::<LanguagesService>() {
            return Ok(service);
        }
        let data = ctx.extensions.get::<DataDir>().ok_or_else(|| {
            CmdError::internal("language settings need the data folder (bmcp_languages::install)")
        })?;
        Ok(ctx
            .extensions
            .get_or_insert_with(|| LanguagesService::new(data.config(), default_locales_dir())))
    }

    /// Connect the browser (done once by the app).
    pub fn set_backend(&self, backend: Arc<dyn LanguageBackend>) {
        *self.backend.write().unwrap_or_else(|e| e.into_inner()) = Some(backend);
    }

    pub fn backend(&self) -> Option<Arc<dyn LanguageBackend>> {
        self.backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn settings(&self) -> LanguageSettings {
        lock(&self.settings).clone()
    }

    /// Store new settings (written to `languages.json` before they count).
    pub fn store(&self, settings: LanguageSettings) -> Result<(), CmdError> {
        settings.save(&self.config_dir).map_err(|err| {
            CmdError::new(
                ErrorCode::Internal,
                format!(
                    "cannot write {}: {err}",
                    LanguageSettings::path_in(&self.config_dir).display()
                ),
            )
        })?;
        *lock(&self.settings) = settings;
        Ok(())
    }

    pub fn started_locale(&self) -> Option<&str> {
        self.started_locale.as_deref()
    }

    pub fn locales_dir(&self) -> &Path {
        &self.locales_dir
    }

    /// Chromium locales shipped next to the executable.
    pub fn available_locales(&self) -> Vec<String> {
        available_locales(&self.locales_dir)
    }

    /// The website languages in effect now.
    pub fn effective_web(&self) -> Vec<String> {
        self.settings().effective_web(&system_language())
    }
}
