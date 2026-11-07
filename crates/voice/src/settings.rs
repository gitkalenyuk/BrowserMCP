//! Voice settings in `data/config/voice.json`, and the API keys the engines use.
//!
//! Keys are plain JSON, like the AI provider keys (the user chose no encryption, see
//! docs/plan.md "Секрети"). OpenAI and Groq also reuse the key of the AI provider of the same
//! name in `data/config/ai.json` when the voice settings have none of their own.

use std::path::{Path, PathBuf};

use bmcp_core::{CmdError, config_file};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Speech-to-text engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EngineId {
    /// whisper.cpp on this computer (whisper-server sidecar): offline, free, private.
    #[default]
    Local,
    /// OpenAI `/audio/transcriptions` (gpt-4o-transcribe, gpt-4o-mini-transcribe, whisper-1).
    Openai,
    /// Groq, OpenAI-compatible (whisper-large-v3-turbo, whisper-large-v3).
    Groq,
    /// ElevenLabs Scribe.
    Elevenlabs,
    /// Deepgram (nova-3), live over WebSocket.
    Deepgram,
    /// Any OpenAI-compatible `/audio/transcriptions` endpoint.
    Custom,
}

impl EngineId {
    pub const ALL: [EngineId; 6] = [
        EngineId::Local,
        EngineId::Openai,
        EngineId::Groq,
        EngineId::Elevenlabs,
        EngineId::Deepgram,
        EngineId::Custom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::Local => "local",
            EngineId::Openai => "openai",
            EngineId::Groq => "groq",
            EngineId::Elevenlabs => "elevenlabs",
            EngineId::Deepgram => "deepgram",
            EngineId::Custom => "custom",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EngineId::Local => "Local Whisper",
            EngineId::Openai => "OpenAI",
            EngineId::Groq => "Groq",
            EngineId::Elevenlabs => "ElevenLabs Scribe",
            EngineId::Deepgram => "Deepgram",
            EngineId::Custom => "Custom (OpenAI-compatible)",
        }
    }

    /// Default API root (empty: local / custom have none).
    pub fn default_base_url(self) -> &'static str {
        match self {
            EngineId::Openai => "https://api.openai.com/v1",
            EngineId::Groq => "https://api.groq.com/openai/v1",
            EngineId::Elevenlabs => "https://api.elevenlabs.io",
            EngineId::Deepgram => "https://api.deepgram.com",
            EngineId::Local | EngineId::Custom => "",
        }
    }

    /// Models offered in Settings (the first is the default).
    pub fn models(self) -> &'static [&'static str] {
        match self {
            EngineId::Openai => &["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"],
            EngineId::Groq => &["whisper-large-v3-turbo", "whisper-large-v3"],
            EngineId::Elevenlabs => &["scribe_v1"],
            EngineId::Deepgram => &["nova-3"],
            EngineId::Local | EngineId::Custom => &[],
        }
    }

    /// Does it need an API key?
    pub fn needs_key(self) -> bool {
        !matches!(self, EngineId::Local | EngineId::Custom)
    }
}

/// Push-to-talk (hold the key) or toggle (tap to start, tap to stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VoiceMode {
    #[default]
    Hold,
    Tap,
}

/// One cloud engine: its key, model and (rarely) API root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// API root; `None` = the engine's own (required for `custom`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Local Whisper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettings {
    /// Model id from the catalog (`base`, `large-v3-turbo-q5_0`…); `None` = the best one
    /// installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// CPU threads for whisper-server; `None` = its default (4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Stop the whisper-server process after this many idle minutes (frees its memory).
    pub idle_minutes: u32,
}

impl Default for LocalSettings {
    fn default() -> Self {
        LocalSettings {
            model: None,
            threads: None,
            idle_minutes: 5,
        }
    }
}

/// The whole `voice.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Dictation on (the hotkey and the microphone buttons work).
    pub enabled: bool,
    pub engine: EngineId,
    pub mode: VoiceMode,
    /// Key that starts dictation while a prompt has focus: `Space` (only while the prompt is
    /// empty, hold mode needs key repeat for the warm-up) or a combo like `Ctrl+Space`,
    /// `Alt+V`.
    pub hotkey: String,
    /// Send the prompt when you let go (hold mode) if the transcript has at least 3 words.
    pub auto_submit_hold: bool,
    /// Send the prompt when you stop (tap mode) if the transcript has at least 3 words.
    pub auto_submit_tap: bool,
    /// `""` = the UI language, `"auto"` = detect, else a Whisper language code (`uk`, `en`…).
    pub language: String,
    /// Pass the current page's title and site to the engine as a vocabulary hint.
    pub page_hint: bool,
    pub local: LocalSettings,
    pub openai: EngineSettings,
    pub groq: EngineSettings,
    pub elevenlabs: EngineSettings,
    pub deepgram: EngineSettings,
    pub custom: EngineSettings,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        VoiceSettings {
            enabled: true,
            engine: EngineId::Local,
            mode: VoiceMode::Hold,
            hotkey: "Space".into(),
            auto_submit_hold: false,
            auto_submit_tap: true,
            language: String::new(),
            page_hint: true,
            local: LocalSettings::default(),
            openai: EngineSettings::default(),
            groq: EngineSettings::default(),
            elevenlabs: EngineSettings::default(),
            deepgram: EngineSettings::default(),
            custom: EngineSettings::default(),
        }
    }
}

impl VoiceSettings {
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("voice.json")
    }

    pub fn load(config_dir: &Path) -> Result<Self, CmdError> {
        config_file::load(&Self::path_in(config_dir))
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), CmdError> {
        config_file::save(&Self::path_in(config_dir), self)
    }

    /// The settings of a cloud engine (`None` for local).
    pub fn engine(&self, id: EngineId) -> Option<&EngineSettings> {
        match id {
            EngineId::Local => None,
            EngineId::Openai => Some(&self.openai),
            EngineId::Groq => Some(&self.groq),
            EngineId::Elevenlabs => Some(&self.elevenlabs),
            EngineId::Deepgram => Some(&self.deepgram),
            EngineId::Custom => Some(&self.custom),
        }
    }

    pub fn engine_mut(&mut self, id: EngineId) -> Option<&mut EngineSettings> {
        match id {
            EngineId::Local => None,
            EngineId::Openai => Some(&mut self.openai),
            EngineId::Groq => Some(&mut self.groq),
            EngineId::Elevenlabs => Some(&mut self.elevenlabs),
            EngineId::Deepgram => Some(&mut self.deepgram),
            EngineId::Custom => Some(&mut self.custom),
        }
    }
}

/// Where an engine's key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// The voice settings (`voice.json`).
    Voice,
    /// The AI provider of the same name (Settings → AI, `ai.json`).
    AiProvider,
}

/// A key and an API root borrowed from `ai.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderKey {
    pub api_key: String,
    pub base_url: Option<String>,
}

/// The key of the AI provider `preset` (`openai`, `groq`) in `ai.json`: the built-in one
/// (`providers.<preset>`), else the first of the user's providers made from that template.
/// Read on each use, so a key added in Settings → AI works at once.
pub fn ai_provider_key(config_dir: &Path, preset: &str) -> Option<ProviderKey> {
    let text = std::fs::read_to_string(config_dir.join("ai.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let from = |v: &serde_json::Value| {
        let key = v.get("api_key")?.as_str()?.trim();
        (!key.is_empty()).then(|| ProviderKey {
            api_key: key.to_string(),
            base_url: v
                .get("base_url")
                .and_then(|u| u.as_str())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string),
        })
    };
    if let Some(found) = json
        .get("providers")
        .and_then(|p| p.get(preset))
        .and_then(from)
    {
        return Some(found);
    }
    json.get("user_providers")?
        .as_array()?
        .iter()
        .filter(|p| p.get("template").and_then(|t| t.as_str()) == Some(preset))
        .find_map(from)
}

/// Normalize a language setting: `""` (follow the UI), `auto`, or a lowercase code.
pub fn normalize_language(lang: &str) -> Result<String, CmdError> {
    let lang = lang.trim().to_ascii_lowercase().replace('_', "-");
    if lang.is_empty() || lang == "auto" {
        return Ok(lang);
    }
    // Whisper uses ISO 639-1 codes (plus `haw`, `yue`); accept `uk-UA` too.
    let base = lang.split('-').next().unwrap_or("");
    if (2..=3).contains(&base.len()) && base.chars().all(|c| c.is_ascii_lowercase()) {
        Ok(base.to_string())
    } else {
        Err(CmdError::invalid_params(format!(
            "language {lang:?} is not a language code (use auto, uk, en, de…)"
        )))
    }
}

/// Normalize a hotkey: `ctrl+space` → `Ctrl+Space`; modifiers Ctrl / Alt / Shift / Meta in
/// that order, then one key.
pub fn normalize_hotkey(hotkey: &str) -> Result<String, CmdError> {
    let bad = || {
        CmdError::invalid_params(format!(
            "hotkey {hotkey:?} is not a key combination (examples: Space, Ctrl+Space, Alt+V, F8)"
        ))
    };
    let parts: Vec<&str> = hotkey
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let (key, mods) = parts.split_last().ok_or_else(bad)?;
    let mut flags = [false; 4];
    for m in mods {
        let i = match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => 0,
            "alt" | "option" => 1,
            "shift" => 2,
            "meta" | "cmd" | "win" | "super" => 3,
            _ => return Err(bad()),
        };
        flags[i] = true;
    }
    let key = match key.to_ascii_lowercase().as_str() {
        "space" | " " => "Space".to_string(),
        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => k.to_ascii_uppercase(),
        k if k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => {
            k.to_ascii_uppercase()
        }
        "backquote" | "`" => "Backquote".to_string(),
        _ => return Err(bad()),
    };
    // A plain letter or digit would steal typing; Space alone is the documented exception.
    let plain = !flags.iter().any(|f| *f);
    if plain && key != "Space" && !key.starts_with('F') {
        return Err(bad().with_hint("Use Space, a function key, or a combo such as Alt+V."));
    }
    let names = ["Ctrl", "Alt", "Shift", "Meta"];
    let mut out: Vec<String> = names
        .iter()
        .zip(flags)
        .filter(|(_, on)| *on)
        .map(|(n, _)| (*n).to_string())
        .collect();
    out.push(key);
    Ok(out.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = VoiceSettings::load(dir.path()).unwrap();
        assert_eq!(s, VoiceSettings::default());
        assert_eq!(s.hotkey, "Space");
        assert!(s.auto_submit_tap && !s.auto_submit_hold);
        let mut s2 = s.clone();
        s2.engine = EngineId::Groq;
        s2.groq.api_key = Some("gsk_x".into());
        s2.save(dir.path()).unwrap();
        assert_eq!(VoiceSettings::load(dir.path()).unwrap(), s2);
        // Partial files keep the defaults of everything else.
        std::fs::write(
            VoiceSettings::path_in(dir.path()),
            r#"{"engine":"deepgram"}"#,
        )
        .unwrap();
        let s3 = VoiceSettings::load(dir.path()).unwrap();
        assert_eq!(s3.engine, EngineId::Deepgram);
        assert_eq!(s3.local.idle_minutes, 5);
    }

    #[test]
    fn borrows_keys_from_ai_json() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(ai_provider_key(dir.path(), "openai"), None);
        std::fs::write(
            dir.path().join("ai.json"),
            r#"{"providers":{"openai":{"model":"x"},"groq":{"api_key":"gsk_1"}},
                "user_providers":[{"id":"w","name":"W","api":"openai","template":"openai",
                  "api_key":"sk-work","base_url":"https://proxy.test/v1"}]}"#,
        )
        .unwrap();
        assert_eq!(
            ai_provider_key(dir.path(), "groq"),
            Some(ProviderKey {
                api_key: "gsk_1".into(),
                base_url: None
            })
        );
        // The built-in OpenAI entry has no key: the user's OpenAI provider is used.
        assert_eq!(
            ai_provider_key(dir.path(), "openai"),
            Some(ProviderKey {
                api_key: "sk-work".into(),
                base_url: Some("https://proxy.test/v1".into())
            })
        );
    }

    #[test]
    fn languages_and_hotkeys() {
        assert_eq!(normalize_language(" UK_ua ").unwrap(), "uk");
        assert_eq!(normalize_language("auto").unwrap(), "auto");
        assert_eq!(normalize_language("").unwrap(), "");
        assert_eq!(normalize_language("haw").unwrap(), "haw");
        assert!(normalize_language("ukrainian").is_err());
        assert_eq!(normalize_hotkey("space").unwrap(), "Space");
        assert_eq!(normalize_hotkey("ctrl+space").unwrap(), "Ctrl+Space");
        assert_eq!(normalize_hotkey("alt + v").unwrap(), "Alt+V");
        assert_eq!(normalize_hotkey("Shift+Ctrl+F8").unwrap(), "Ctrl+Shift+F8");
        assert_eq!(normalize_hotkey("f8").unwrap(), "F8");
        assert!(normalize_hotkey("v").is_err());
        assert!(normalize_hotkey("Hyper+V").is_err());
        assert!(normalize_hotkey("").is_err());
    }
}
