//! Languages of the browser (Settings → General → Language):
//!
//! * **Website languages** — the ordered list sites see as `Accept-Language` and
//!   `navigator.languages` (Chromium prefs `intl.selected_languages` + `intl.accept_languages`).
//!   Applied live to every profile's request context and to contexts created later. Empty =
//!   automatic: the interface language the UI reported, then English (`uk, en`).
//! * **Browser language** — Chromium's own locale (`CefSettings.locale`: its context menu,
//!   dialogs, error pages). Read once at startup, so a change needs a restart. Only locales
//!   whose `locales/<name>.pak` ships next to the executable can be chosen; "system" picks the
//!   shipped one closest to the Windows display language ([`startup_locale`]).
//! * **Spellcheck dictionaries** — prefs `spellcheck.dictionaries` and
//!   `browser.enable_spellchecking`, set on every context when this Chromium build has them.
//!
//! **Precedence** ([`accept_language_for`]): a profile's own language — its stealth identity
//! (`identity_assign`, fitted to the proxy's country with `fit_identity`) or
//! `profile.create {accept_language}` — always wins over the global list, so the language a
//! site sees keeps matching the proxy's IP. The global list fills in for every other profile.
//! Identity tabs also override `navigator.languages` through CDP (bmcp-identity).
//!
//! Commands: `browser.languages_get`, `browser.languages_set`; stored in
//! `data/config/languages.json`. The CEF crate reads the same file before Chromium starts
//! (locale, first Accept-Language) and implements [`LanguageBackend`] for live changes.

pub mod commands;
pub mod config;
pub mod service;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use bmcp_core::Registry;

pub use commands::LanguagesInfo;
pub use config::{
    LanguageSettings, LanguageSource, accept_language_for, automatic_web, available_locales,
    default_locales_dir, normalize_list, normalize_tag, startup_locale, system_language,
};
pub use service::{BackendLanguages, LanguageBackend, LanguagesService, ProfileLanguage};

/// Register `browser.languages_get` and `browser.languages_set`.
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::LanguagesGet>()
        .add::<commands::LanguagesSet>();
}

/// Put the service (settings in `config_dir`, Chromium locales next to the executable) into
/// the registry's extensions; the app then connects the browser with
/// [`LanguagesService::set_backend`].
pub fn install(registry: &Registry, config_dir: impl Into<PathBuf>) -> Arc<LanguagesService> {
    registry
        .extensions()
        .insert(LanguagesService::new(config_dir, default_locales_dir()))
}
