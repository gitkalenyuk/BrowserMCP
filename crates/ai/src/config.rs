//! Provider presets and the user's settings in `data/config/ai.json`.
//!
//! Keys are stored as plain JSON (the user chose no encryption, see docs/plan.md "Секрети"):
//!
//! ```json
//! { "providers": {
//!     "anthropic": { "api_key": "sk-ant-…", "model": "claude-opus-5-5" },
//!     "ollama": { "model": "qwen3:8b", "models": ["qwen3:8b"] },
//!     "custom": { "base_url": "http://10.0.0.5:8000/v1", "api_key": "…", "model": "my-model" },
//!     "claude-code": { "path": "C:\\Users\\me\\.local\\bin\\claude.exe" }
//! } }
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Api;

/// How BrowserMCP talks to a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// HTTP model API; BrowserMCP runs the agent loop itself.
    Api(Api),
    /// Claude Code CLI (`claude -p --input-format stream-json …`).
    ClaudeCode,
    /// Codex CLI (`codex app-server`, fallback `codex exec --json`).
    Codex,
}

/// A built-in provider.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// Default API root; `""` = the user must set one (Custom).
    pub base_url: &'static str,
    /// Whether an API key is required.
    pub needs_key: bool,
    /// Default model; `""` = pick from `models` / set with `ai.provider_set`.
    pub model: &'static str,
    /// Suggested models (replaced by the live list after `ai.provider_test`).
    pub models: &'static [&'static str],
}

const fn api(
    id: &'static str,
    name: &'static str,
    api: Api,
    base_url: &'static str,
    needs_key: bool,
    model: &'static str,
    models: &'static [&'static str],
) -> Preset {
    Preset {
        id,
        name,
        kind: Kind::Api(api),
        base_url,
        needs_key,
        model,
        models,
    }
}

/// Every provider, in the order the UI lists them.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "claude-code",
        name: "Claude Code",
        kind: Kind::ClaudeCode,
        base_url: "",
        needs_key: false,
        model: "",
        models: &["opus", "sonnet", "haiku", "fable"],
    },
    Preset {
        id: "codex",
        name: "Codex",
        kind: Kind::Codex,
        base_url: "",
        needs_key: false,
        model: "",
        models: &[],
    },
    api(
        "anthropic",
        "Anthropic",
        Api::Anthropic,
        "https://api.anthropic.com",
        true,
        "claude-opus-5-5",
        &[
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
        ],
    ),
    api(
        "openai",
        "OpenAI",
        Api::OpenAi,
        "https://api.openai.com/v1",
        true,
        "",
        &[],
    ),
    api(
        "gemini",
        "Google Gemini",
        Api::Gemini,
        "https://generativelanguage.googleapis.com",
        true,
        "",
        &[],
    ),
    api(
        "openrouter",
        "OpenRouter",
        Api::OpenAi,
        "https://openrouter.ai/api/v1",
        true,
        "",
        &[],
    ),
    api(
        "mistral",
        "Mistral",
        Api::OpenAi,
        "https://api.mistral.ai/v1",
        true,
        "",
        &[],
    ),
    api(
        "deepseek",
        "DeepSeek",
        Api::OpenAi,
        "https://api.deepseek.com/v1",
        true,
        "deepseek-chat",
        &["deepseek-chat", "deepseek-reasoner"],
    ),
    api(
        "xai",
        "xAI",
        Api::OpenAi,
        "https://api.x.ai/v1",
        true,
        "",
        &[],
    ),
    api(
        "groq",
        "Groq",
        Api::OpenAi,
        "https://api.groq.com/openai/v1",
        true,
        "",
        &[],
    ),
    api(
        "ollama",
        "Ollama",
        Api::OpenAi,
        "http://localhost:11434/v1",
        false,
        "",
        &[],
    ),
    api(
        "lmstudio",
        "LM Studio",
        Api::OpenAi,
        "http://localhost:1234/v1",
        false,
        "",
        &[],
    ),
    api(
        "custom",
        "Custom (OpenAI-compatible)",
        Api::OpenAi,
        "",
        false,
        "",
        &[],
    ),
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// What the user changed for one provider. Unset fields fall back to the preset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// CLI providers: explicit path of the `claude` / `codex` binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Model list from the last successful `ai.provider_test`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// CLI providers: default permission mode, sandbox and folders of new chats
    /// (`ai.agent_access_set`).
    #[serde(
        default,
        skip_serializing_if = "crate::cli::access::AccessSettings::is_default"
    )]
    pub access: crate::cli::access::AccessSettings,
}

/// The whole `ai.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderSettings>,
    /// Provider used by background AI features (self-healing, `page.extract`) when the call
    /// names none. `None` = the first configured one (see `AiService::default_provider`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
}

impl AiConfig {
    /// Read `path`; a missing or broken file gives the defaults (a broken one is logged).
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(%err, path = %path.display(), "ignoring unreadable ai.json");
                AiConfig::default()
            }),
            Err(_) => AiConfig::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write to a temp file first so a crash never leaves a half-written config.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    pub fn settings(&self, id: &str) -> ProviderSettings {
        self.providers.get(id).cloned().unwrap_or_default()
    }
}

/// A preset merged with the user's settings: what a session actually uses.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub preset: &'static Preset,
    pub base_url: String,
    pub api_key: Option<String>,
    /// Configured model, else the preset default, else the first known model.
    pub model: Option<String>,
    pub models: Vec<String>,
    /// CLI providers: the binary, when found.
    pub binary: Option<PathBuf>,
}

impl Resolved {
    pub fn new(preset: &'static Preset, settings: ProviderSettings) -> Self {
        let models = if settings.models.is_empty() {
            preset.models.iter().map(|m| m.to_string()).collect()
        } else {
            settings.models.clone()
        };
        let model = settings
            .model
            .filter(|m| !m.is_empty())
            .or_else(|| (!preset.model.is_empty()).then(|| preset.model.to_string()))
            .or_else(|| match preset.kind {
                // CLI agents have their own default model.
                Kind::Api(_) => models.first().cloned(),
                _ => None,
            });
        let binary = match preset.kind {
            Kind::ClaudeCode => crate::cli::find_claude(settings.path.as_deref()),
            Kind::Codex => crate::cli::find_codex(settings.path.as_deref()),
            Kind::Api(_) => None,
        };
        Resolved {
            preset,
            base_url: settings
                .base_url
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| preset.base_url.to_string()),
            api_key: settings.api_key.filter(|k| !k.is_empty()),
            model,
            models,
            binary,
        }
    }

    /// Ready to use: API providers have a URL (and a key when required); CLIs are installed.
    pub fn configured(&self) -> bool {
        match self.preset.kind {
            Kind::Api(_) => {
                !self.base_url.is_empty() && (!self.preset.needs_key || self.api_key.is_some())
            }
            Kind::ClaudeCode | Kind::Codex => self.binary.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_unique_and_complete() {
        let mut ids: Vec<_> = PRESETS.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), PRESETS.len());
        for id in [
            "anthropic",
            "openai",
            "gemini",
            "openrouter",
            "mistral",
            "deepseek",
            "xai",
            "groq",
            "ollama",
            "lmstudio",
            "custom",
            "claude-code",
            "codex",
        ] {
            assert!(preset(id).is_some(), "missing preset {id}");
        }
        assert_eq!(
            preset("ollama").unwrap().base_url,
            "http://localhost:11434/v1"
        );
        assert_eq!(
            preset("lmstudio").unwrap().base_url,
            "http://localhost:1234/v1"
        );
    }

    #[test]
    fn save_load_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/ai.json");
        assert_eq!(AiConfig::load(&path), AiConfig::default());

        let mut config = AiConfig::default();
        config.providers.insert(
            "openai".into(),
            ProviderSettings {
                api_key: Some("sk-test".into()),
                models: vec!["m1".into(), "m2".into()],
                ..Default::default()
            },
        );
        config.save(&path).unwrap();
        let loaded = AiConfig::load(&path);
        assert_eq!(loaded, config);

        let openai = Resolved::new(preset("openai").unwrap(), loaded.settings("openai"));
        assert!(openai.configured());
        assert_eq!(openai.model.as_deref(), Some("m1"));
        assert_eq!(openai.base_url, "https://api.openai.com/v1");

        // No key → not configured; local providers need none.
        let gemini = Resolved::new(preset("gemini").unwrap(), loaded.settings("gemini"));
        assert!(!gemini.configured());
        let ollama = Resolved::new(preset("ollama").unwrap(), loaded.settings("ollama"));
        assert!(ollama.configured());
        // Custom has no URL until the user sets one.
        let custom = Resolved::new(preset("custom").unwrap(), loaded.settings("custom"));
        assert!(!custom.configured());
    }
}
