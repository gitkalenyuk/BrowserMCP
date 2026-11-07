//! Languages in CEF (rules and storage: `bmcp-languages`).
//!
//! * Startup ([`startup`]): `data/config/languages.json` gives `CefSettings.locale` (Chromium's
//!   own UI language, only read by `cef::initialize`) and the first website list.
//! * Website languages: the global list is kept here ([`set`]) and written as the prefs
//!   `intl.selected_languages` + `intl.accept_languages` on every request context ([`apply`]):
//!   the global context once it is initialized, each profile context in
//!   `on_request_context_initialized` and after every `profile.*` change, and all of them
//!   when `browser.languages_set` changes the list. Chromium follows the prefs live (header
//!   and `navigator.languages` of the next load).
//! * Precedence ([`bmcp_languages::accept_language_for`]): a profile's own Accept-Language
//!   (its stealth identity, fitted to the proxy's country, or `profile.create
//!   {accept_language}`) wins: such a context gets its own list, never the global one.
//! * Pitfalls (measured live, see tests/e2e/tests/languages.test.ts):
//!   - `intl.accept_languages` alone does not stick: Chromium recomputes it from
//!     `intl.selected_languages` (the user's list) whenever a browser is created, so both are
//!     set — for identity profiles too, or a new tab would fall back to the default list;
//!   - the global list must NOT go into `CefSettings.accept_language_list` /
//!     `RequestContextSettings.accept_language_list`: CEF prefers those and writes them back
//!     into the prefs, so the global list set there never changed at runtime. Only a
//!     profile's own (identity) language is passed at context creation ([`own_accept`]), as
//!     before; the two prefs [`apply`] sets afterwards do win over it: re-fitting or removing
//!     a profile's language reaches its next tab without a restart (e2e "keeps a profile's own
//!     language and follows changes to it live");
//!   - an empty `CefSettings.locale` means en-US in CEF, not the Windows language: "system"
//!     is resolved to a shipped locale here ([`bmcp_languages::startup_locale`]).
//! * Spellcheck: `spellcheck.dictionaries` + `browser.enable_spellchecking` on every context,
//!   only once a list was saved (otherwise Chromium's default stays).
//!
//! UI thread only (except [`startup`], which runs before CEF exists).

use std::path::Path;
use std::sync::Mutex;

use bmcp_languages::{
    BackendLanguages, LanguageSettings, ProfileLanguage, accept_language_for, available_locales,
    default_locales_dir, startup_locale, system_language,
};
use cef::*;
use serde_json::{Value, json};

use crate::bridge::with_ui;
use crate::profiles::{launch, set_pref};

const ACCEPT_LANGUAGES: &str = "intl.accept_languages";
const SELECTED_LANGUAGES: &str = "intl.selected_languages";
const DICTIONARIES: &str = "spellcheck.dictionaries";
const SPELLCHECK_ON: &str = "browser.enable_spellchecking";

/// The languages in effect for profiles without their own.
#[derive(Debug, Clone, Default)]
struct Current {
    /// `uk,en` (no q-values: Chromium adds them to the header).
    accept: String,
    /// `None` = leave Chromium's spellcheck settings alone.
    spellcheck: Option<Vec<String>>,
}

static CURRENT: Mutex<Option<Current>> = Mutex::new(None);

fn current() -> Current {
    CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// What `cef::initialize` needs from the saved settings.
pub(crate) struct Startup {
    /// `CefSettings.locale`; `None` = CEF's default (en-US).
    pub locale: Option<String>,
}

/// Read `languages.json` in `config_dir` before CEF starts and remember the website list.
pub(crate) fn startup(config_dir: &Path) -> Startup {
    let settings = LanguageSettings::load(config_dir);
    let system = system_language();
    set(settings.effective_web(&system), settings.spellcheck.clone());
    let available = available_locales(&default_locales_dir());
    Startup {
        locale: startup_locale(settings.browser_locale.as_deref(), &system, &available),
    }
}

/// Remember the global website languages and spellcheck dictionaries.
fn set(web: Vec<String>, spellcheck: Option<Vec<String>>) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Current {
        accept: web.join(","),
        spellcheck,
    });
}

/// The profile's own Accept-Language (from `profile.*` settings: explicit or its identity's).
fn own_language(profile: &str) -> Option<String> {
    with_ui(|ui| {
        ui.profiles
            .settings
            .get(profile)
            .and_then(|s| s.prefs.accept_language.clone())
    })
}

/// `RequestContextSettings.accept_language_list` of a new context of `profile`: its own
/// language (identity), or empty so the pref set by [`apply`] rules (see the pitfall above).
/// UI thread.
pub(crate) fn own_accept(profile: &str) -> String {
    own_language(profile)
        .map(|l| bmcp_profiles::prefs::strip_q_values(&l))
        .unwrap_or_default()
}

/// Set the website languages of `context` of `profile`: its own (stealth identity) or the
/// global list, as both language prefs (see the pitfalls above). Spellcheck is set on every
/// context. UI thread.
pub(crate) fn apply(context: &RequestContext, profile: &str) {
    let current = current();
    let own = own_language(profile).map(|l| bmcp_profiles::prefs::strip_q_values(&l));
    let (accept, source) = accept_language_for(own.as_deref(), &current.accept);
    if !accept.is_empty() {
        tracing::debug!(profile, ?source, accept, "website languages");
        set_pref(context, SELECTED_LANGUAGES, &json!(accept));
        set_pref(context, ACCEPT_LANGUAGES, &json!(accept));
    }
    if let Some(dictionaries) = &current.spellcheck {
        set_pref(context, DICTIONARIES, &json!(dictionaries));
        set_pref(context, SPELLCHECK_ON, &json!(!dictionaries.is_empty()));
    }
}

/// The global context finished initializing (`on_context_initialized`). UI thread.
pub(crate) fn apply_to_global() {
    if let Some(global) = request_context_get_global_context() {
        apply(&global, &launch().profile);
    }
}

/// `browser.languages_set`: new global languages for every live context. UI thread.
pub(crate) fn apply_everywhere(web: Vec<String>, spellcheck: Option<Vec<String>>) {
    set(web, spellcheck);
    let contexts: Vec<(String, RequestContext)> = with_ui(|ui| {
        ui.profiles
            .contexts
            .iter()
            .map(|(key, context)| (key.name(), context.clone()))
            .collect()
    });
    apply_to_global();
    for (profile, context) in contexts {
        apply(&context, &profile);
    }
}

/// What `browser.languages_get` reports from the browser. UI thread.
pub(crate) fn report() -> BackendLanguages {
    let global = request_context_get_global_context();
    let supported = global.as_ref().map(|context| {
        let name = CefString::from(DICTIONARIES);
        context.can_set_preference(Some(&name)) == 1
    });
    let current = global.as_ref().and_then(|context| {
        let on = get_pref(context, SPELLCHECK_ON)?.as_bool().unwrap_or(true);
        if !on {
            return Some(Vec::new());
        }
        let list = get_pref(context, DICTIONARIES)?;
        Some(
            list.as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        )
    });
    let own_language = with_ui(|ui| {
        ui.profiles
            .settings
            .iter()
            .filter_map(|(id, s)| {
                Some(ProfileLanguage {
                    profile: id.clone(),
                    accept_language: s.prefs.accept_language.clone()?,
                })
            })
            .collect()
    });
    BackendLanguages {
        spellcheck_supported: supported,
        spellcheck_current: current,
        own_language,
    }
}

/// A preference of `context` as JSON (`None` when it is not registered).
fn get_pref(context: &RequestContext, name: &str) -> Option<Value> {
    let name = CefString::from(name);
    if context.has_preference(Some(&name)) != 1 {
        return None;
    }
    let mut value = context.preference(Some(&name))?;
    let text =
        CefString::from(&write_json(Some(&mut value), JsonWriterOptions::default())).to_string();
    serde_json::from_str(&text).ok()
}
