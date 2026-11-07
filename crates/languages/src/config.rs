//! The stored language settings (`data/config/languages.json`) and the pure rules around
//! them: language tags, the automatic website list, which value a profile ends up with, the
//! Chromium locales next to the executable.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File name inside `data/config`.
pub const FILE: &str = "languages.json";

/// What `browser.languages_set` stores. Every field has a default, so an old or partial file
/// still loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LanguageSettings {
    /// Languages websites are told (`Accept-Language`, `navigator.languages`), most preferred
    /// first, e.g. `["uk", "en"]`. Empty = automatic: the interface language, then English.
    pub web: Vec<String>,
    /// Chromium's own locale (its context menu, dialogs, error pages), e.g. `uk` or `en-US`.
    /// `None` = the Windows display language. Read at startup only.
    pub browser_locale: Option<String>,
    /// Spellcheck dictionaries, e.g. `["uk", "en-US"]`; `[]` = spellcheck off; `None` = never
    /// set, Chromium's own default is left alone.
    pub spellcheck: Option<Vec<String>>,
    /// The browser UI's interface language as the UI last reported it (`en`, `uk`): the first
    /// language of the automatic website list. `None` = the Windows display language.
    pub ui_language: Option<String>,
}

impl LanguageSettings {
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join(FILE)
    }

    /// The settings in `config_dir`; defaults when the file is missing or unreadable (a broken
    /// file must never stop the browser from starting).
    pub fn load(config_dir: &Path) -> LanguageSettings {
        let path = Self::path_in(config_dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return LanguageSettings::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), %err, "languages.json ignored");
            LanguageSettings::default()
        })
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(Self::path_in(config_dir), text)
    }

    /// The website languages in effect: the stored list, or the automatic one built from the
    /// interface language (`system` = the Windows display language, when the UI never said).
    pub fn effective_web(&self, system: &str) -> Vec<String> {
        if !self.web.is_empty() {
            return self.web.clone();
        }
        automatic_web(self.ui_language.as_deref().unwrap_or(system))
    }
}

/// The automatic website languages for an interface language: that language, its base
/// language when it has a region, then English (`uk-UA` → `uk-UA, uk, en`; `en` → `en-US, en`).
pub fn automatic_web(ui_language: &str) -> Vec<String> {
    let tag = normalize_tag(ui_language).unwrap_or_else(|| "en".to_string());
    let base = base_of(&tag).to_string();
    let mut list = Vec::new();
    if tag == "en" {
        list.push("en-US".to_string());
    } else {
        list.push(tag.clone());
    }
    for extra in [base, "en".to_string()] {
        if !list.iter().any(|l| l.eq_ignore_ascii_case(&extra)) {
            list.push(extra);
        }
    }
    list
}

/// `pt-BR` → `pt`.
pub fn base_of(tag: &str) -> &str {
    tag.split('-').next().unwrap_or(tag)
}

/// A BCP 47 language tag in its usual spelling (`EN-us` → `en-US`, `zh-hant` → `zh-Hant`,
/// `uk_UA` → `uk-UA`), or `None` when it is not one. Accepts a language of 2-3 letters and
/// subtags of 2-8 letters or digits; Chromium's own list syntax (commas, q-values) is refused.
pub fn normalize_tag(tag: &str) -> Option<String> {
    let tag = tag.trim().replace('_', "-");
    let mut parts = tag.split('-');
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = language.to_ascii_lowercase();
    for part in parts {
        if !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        out.push('-');
        match part.len() {
            // Region (UA, BR) or a UN M.49 area code (419).
            2 => out.push_str(&part.to_ascii_uppercase()),
            3 if part.bytes().all(|b| b.is_ascii_digit()) => out.push_str(part),
            // Script (Hant, Latn).
            4 => {
                let lower = part.to_ascii_lowercase();
                out.push_str(&lower[..1].to_ascii_uppercase());
                out.push_str(&lower[1..]);
            }
            _ => out.push_str(&part.to_ascii_lowercase()),
        }
    }
    Some(out)
}

/// Normalize a list of tags: each one checked, duplicates (any case) dropped, order kept.
/// The error names the first tag that is not a language.
pub fn normalize_list(tags: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let normalized = normalize_tag(tag).ok_or_else(|| {
            format!("{tag:?} is not a language tag (use codes like uk, en-US, pt-BR)")
        })?;
        if !out.iter().any(|t| t.eq_ignore_ascii_case(&normalized)) {
            out.push(normalized);
        }
    }
    Ok(out)
}

/// Where a request context's `Accept-Language` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LanguageSource {
    /// The profile's own value: `profile.create {accept_language}` or its stealth identity
    /// (`identity_assign`, fitted to the proxy's country by `fit_identity`). It always wins:
    /// a profile that looks like it browses from Germany must keep saying `de-DE`.
    Profile,
    /// "Languages for websites" of Settings → General (`browser.languages_set {web}`).
    Global,
}

/// The precedence rule, in one place: a profile's own language wins over the global list.
/// `own` = the profile's stored Accept-Language (`None` or blank = it has none).
pub fn accept_language_for(own: Option<&str>, global: &str) -> (String, LanguageSource) {
    match own.map(str::trim).filter(|l| !l.is_empty()) {
        Some(own) => (own.to_string(), LanguageSource::Profile),
        None => (global.to_string(), LanguageSource::Global),
    }
}

/// The Chromium locales shipped next to the executable: the names of `locales/*.pak`
/// without their grammatical-gender variants (`uk_FEMININE.pak` belongs to `uk`), sorted.
/// The portable package keeps only `en-US` and `uk` by default (docs/release.md).
pub fn available_locales(locales_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(locales_dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".pak")?;
            let locale = stem.split('_').next().unwrap_or(stem);
            (!locale.is_empty()).then(|| locale.to_string())
        })
        .collect();
    out.sort_by_key(|l| l.to_ascii_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

/// The `CefSettings.locale` to start Chromium with: the saved one, or for "system" (`None`) the
/// shipped locale closest to the Windows display language (`uk-UA` → `uk`, `en-AU` → `en-US`,
/// `pt-BR` → `pt-BR`). `None` = no match: CEF then uses en-US (its default for an empty
/// locale — not the Windows language, hence this lookup).
pub fn startup_locale(saved: Option<&str>, system: &str, available: &[String]) -> Option<String> {
    if let Some(saved) = saved {
        return Some(saved.to_string());
    }
    let find = |tag: &str| {
        available
            .iter()
            .find(|l| l.eq_ignore_ascii_case(tag))
            .cloned()
    };
    let base = base_of(system);
    find(system)
        .or_else(|| find(base))
        .or_else(|| (base == "en").then(|| find("en-US")).flatten())
        .or_else(|| {
            let prefix = format!("{base}-");
            available
                .iter()
                .find(|l| l.to_ascii_lowercase().starts_with(&prefix))
                .cloned()
        })
}

/// `locales/` next to the running executable (where CEF looks for them by default).
pub fn default_locales_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("locales")))
        .unwrap_or_else(|| PathBuf::from("locales"))
}

/// The Windows display language (`uk-UA`), `en-US` when it cannot be read.
pub fn system_language() -> String {
    os_language()
        .and_then(|l| normalize_tag(&l))
        .unwrap_or_else(|| "en-US".to_string())
}

#[cfg(windows)]
fn os_language() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    // LOCALE_NAME_MAX_LENGTH (85 UTF-16 units; the constant lives in another windows-sys module).
    let mut buf = [0u16; 85];
    // SAFETY: plain Win32 calls; the buffer has the LOCALE_NAME_MAX_LENGTH units the API asks for.
    let len = unsafe {
        let lang = GetUserDefaultUILanguage();
        LCIDToLocaleName(u32::from(lang), buf.as_mut_ptr(), buf.len() as i32, 0)
    };
    // `len` counts the terminating NUL; 0 = failure.
    (len > 1).then(|| String::from_utf16_lossy(&buf[..len as usize - 1]))
}

#[cfg(not(windows))]
fn os_language() -> Option<String> {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .ok()?;
    lang.split('.').next().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalized() {
        assert_eq!(normalize_tag("EN-us").as_deref(), Some("en-US"));
        assert_eq!(normalize_tag(" uk_ua ").as_deref(), Some("uk-UA"));
        assert_eq!(normalize_tag("zh-hant-tw").as_deref(), Some("zh-Hant-TW"));
        assert_eq!(normalize_tag("es-419").as_deref(), Some("es-419"));
        assert_eq!(normalize_tag("fil").as_deref(), Some("fil"));
        for bad in ["", "e", "english", "en;q=0.9", "en,de", "en-", "1a", "en-a"] {
            assert_eq!(normalize_tag(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn lists_drop_duplicates_and_name_the_bad_tag() {
        let list = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            normalize_list(&list(&["uk", "EN", "en", "Uk"])).unwrap(),
            ["uk", "en"]
        );
        let err = normalize_list(&list(&["uk", "x y"])).unwrap_err();
        assert!(err.contains("\"x y\""), "{err}");
    }

    #[test]
    fn automatic_list_is_the_interface_language_then_english() {
        assert_eq!(automatic_web("uk"), ["uk", "en"]);
        assert_eq!(automatic_web("uk-UA"), ["uk-UA", "uk", "en"]);
        assert_eq!(automatic_web("en"), ["en-US", "en"]);
        assert_eq!(automatic_web("en-GB"), ["en-GB", "en"]);
        assert_eq!(automatic_web("not a tag"), ["en-US", "en"]);
    }

    #[test]
    fn effective_web_prefers_the_stored_list() {
        let mut s = LanguageSettings::default();
        assert_eq!(s.effective_web("de-DE"), ["de-DE", "de", "en"]);
        s.ui_language = Some("uk".into());
        assert_eq!(s.effective_web("de-DE"), ["uk", "en"]);
        s.web = vec!["pl".into(), "en".into()];
        assert_eq!(s.effective_web("de-DE"), ["pl", "en"]);
    }

    #[test]
    fn a_profiles_own_language_wins() {
        assert_eq!(
            accept_language_for(Some("de-DE,de,en-US,en"), "uk,en"),
            ("de-DE,de,en-US,en".to_string(), LanguageSource::Profile)
        );
        assert_eq!(
            accept_language_for(None, "uk,en"),
            ("uk,en".to_string(), LanguageSource::Global)
        );
        assert_eq!(
            accept_language_for(Some("  "), "uk,en").1,
            LanguageSource::Global
        );
    }

    #[test]
    fn settings_round_trip_and_survive_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        assert_eq!(LanguageSettings::load(&config), LanguageSettings::default());
        let s = LanguageSettings {
            web: vec!["uk".into(), "en".into()],
            browser_locale: Some("uk".into()),
            spellcheck: Some(vec!["uk".into()]),
            ui_language: Some("uk".into()),
        };
        s.save(&config).unwrap();
        assert_eq!(LanguageSettings::load(&config), s);
        // Partial file: the rest is defaulted.
        std::fs::write(config.join(FILE), r#"{"web": ["pl"]}"#).unwrap();
        assert_eq!(LanguageSettings::load(&config).web, ["pl"]);
        // Broken file: defaults, no panic.
        std::fs::write(config.join(FILE), "{not json").unwrap();
        assert_eq!(LanguageSettings::load(&config), LanguageSettings::default());
    }

    #[test]
    fn locales_come_from_the_pak_files() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "en-US.pak",
            "uk.pak",
            "uk_FEMININE.pak",
            "uk_MASCULINE.pak",
            "pt-BR.pak",
            "readme.txt",
        ] {
            std::fs::write(dir.path().join(f), b"").unwrap();
        }
        assert_eq!(available_locales(dir.path()), ["en-US", "pt-BR", "uk"]);
        assert!(available_locales(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn system_locale_maps_to_a_shipped_pak() {
        let paks: Vec<String> = ["en-GB", "en-US", "pt-BR", "uk"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            startup_locale(Some("uk"), "de-DE", &paks).as_deref(),
            Some("uk")
        );
        assert_eq!(startup_locale(None, "uk-UA", &paks).as_deref(), Some("uk"));
        assert_eq!(
            startup_locale(None, "en-GB", &paks).as_deref(),
            Some("en-GB")
        );
        assert_eq!(
            startup_locale(None, "en-AU", &paks).as_deref(),
            Some("en-US")
        );
        assert_eq!(
            startup_locale(None, "pt-PT", &paks).as_deref(),
            Some("pt-BR")
        );
        assert_eq!(startup_locale(None, "de-DE", &paks), None);
        assert_eq!(startup_locale(None, "uk-UA", &[]), None);
    }

    #[test]
    fn system_language_is_a_tag() {
        assert!(normalize_tag(&system_language()).is_some());
    }
}
