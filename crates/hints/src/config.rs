//! Settings: on/off, trigger keys, sites where the keys are off, label alphabet.
//! Stored in `data/config/hints.json` (when the registry has a `DataDir`).

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::labels::DEFAULT_ALPHABET;

/// What a picked hint does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Click the element (trusted mouse input, like page.act click).
    #[default]
    Click,
    /// Open the link in a new background tab (elements without a link are clicked).
    Newtab,
    /// Give the element keyboard focus.
    Focus,
    /// Copy the link URL (or the element's text) to the clipboard.
    Copy,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Click, Mode::Newtab, Mode::Focus, Mode::Copy];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Click => "click",
            Mode::Newtab => "newtab",
            Mode::Focus => "focus",
            Mode::Copy => "copy",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// Trigger keys typed in the page (`KeyboardEvent.key`, one character; case matters, so `F`
/// is Shift+F). `null` = no key for that mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Keys {
    pub click: Option<String>,
    pub newtab: Option<String>,
    pub focus: Option<String>,
    pub copy: Option<String>,
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            click: Some("f".into()),
            newtab: Some("F".into()),
            focus: None,
            copy: None,
        }
    }
}

impl Keys {
    /// `(mode, key)` pairs that are set.
    pub fn bound(&self) -> Vec<(Mode, &str)> {
        [
            (Mode::Click, &self.click),
            (Mode::Newtab, &self.newtab),
            (Mode::Focus, &self.focus),
            (Mode::Copy, &self.copy),
        ]
        .into_iter()
        .filter_map(|(m, k)| k.as_deref().map(|k| (m, k)))
        .collect()
    }

    /// Every key must be exactly one character and used by one mode only.
    pub fn check(&self) -> Result<(), String> {
        let bound = self.bound();
        for (i, (mode, key)) in bound.iter().enumerate() {
            if key.chars().count() != 1 || key.trim().is_empty() {
                return Err(format!(
                    "trigger key for {} must be one character, got {key:?}",
                    mode.as_str()
                ));
            }
            if bound[..i].iter().any(|(_, k)| k == key) {
                return Err(format!("trigger key {key:?} is used twice"));
            }
        }
        Ok(())
    }
}

/// Everything in `hints.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HintsConfig {
    /// Trigger keys work in pages (hints.show works either way).
    pub enabled: bool,
    pub keys: Keys,
    /// Host names where the trigger keys are off (a site's own shortcuts win); a name also
    /// covers its subdomains (`github.com` → `gist.github.com`).
    pub disabled_sites: Vec<String>,
    /// Letters labels are made of.
    pub alphabet: String,
}

impl Default for HintsConfig {
    fn default() -> Self {
        HintsConfig {
            enabled: true,
            keys: Keys::default(),
            disabled_sites: Vec::new(),
            alphabet: DEFAULT_ALPHABET.into(),
        }
    }
}

impl HintsConfig {
    /// `<config dir>/hints.json`.
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("hints.json")
    }

    /// Read the file; a missing or broken file gives the defaults (broken ones are logged).
    pub fn load(path: &Path) -> HintsConfig {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(%err, path = %path.display(), "hints.json is invalid; using defaults");
                HintsConfig::default()
            }),
            Err(_) => HintsConfig::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// The part the page script needs, as it reads it (`config` in `hints.js`).
    pub fn page_json(&self) -> Value {
        let keys: serde_json::Map<String, Value> = self
            .keys
            .bound()
            .into_iter()
            .map(|(m, k)| (m.as_str().to_string(), json!(k)))
            .collect();
        json!({
            "enabled": self.enabled,
            "keys": keys,
            "disabledSites": self.disabled_sites,
        })
    }
}

/// `"https://www.github.com/x"` or `"GitHub.com"` → `"github.com"` (host only, lower case,
/// without a leading `www.`).
pub fn normalize_site(site: &str) -> String {
    let s = site.trim().to_ascii_lowercase();
    let s = s.split_once("://").map_or(s.as_str(), |(_, rest)| rest);
    let host = s.split(['/', '?', '#']).next().unwrap_or_default();
    // Strip a port.
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_page_json() {
        let config = HintsConfig::default();
        assert!(config.enabled);
        assert_eq!(
            config.page_json(),
            json!({ "enabled": true, "keys": { "click": "f", "newtab": "F" }, "disabledSites": [] })
        );
    }

    #[test]
    fn key_rules() {
        assert!(Keys::default().check().is_ok());
        let twice = Keys {
            newtab: Some("f".into()),
            ..Keys::default()
        };
        assert!(twice.check().is_err());
        let long = Keys {
            click: Some("ff".into()),
            ..Keys::default()
        };
        assert!(long.check().is_err());
    }

    #[test]
    fn sites_are_normalized() {
        assert_eq!(normalize_site("https://www.GitHub.com/x?y"), "github.com");
        assert_eq!(normalize_site("localhost:8080"), "localhost");
        assert_eq!(normalize_site("mail.google.com"), "mail.google.com");
    }

    #[test]
    fn file_round_trip_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = HintsConfig::path_in(dir.path());
        assert_eq!(HintsConfig::load(&path), HintsConfig::default());
        let config = HintsConfig {
            enabled: false,
            disabled_sites: vec!["github.com".into()],
            ..HintsConfig::default()
        };
        config.save(&path).unwrap();
        assert_eq!(HintsConfig::load(&path), config);
        // Older/handwritten files may leave fields out (defaults fill them); null unbinds.
        std::fs::write(&path, r#"{"keys": {"click": "g", "focus": null}}"#).unwrap();
        let partial = HintsConfig::load(&path);
        assert_eq!(partial.keys.click.as_deref(), Some("g"));
        assert_eq!(partial.keys.newtab.as_deref(), Some("F"));
        assert_eq!(partial.keys.focus, None);
        assert!(partial.enabled);
    }
}
