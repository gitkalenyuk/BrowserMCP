//! AI providers and the user's settings in `data/config/ai.json`.
//!
//! Two sorts of providers:
//!
//! * **built-in presets** ([`PRESETS`]): Claude Code, Codex, Anthropic, OpenAI, Gemini, … The
//!   user only overrides their key / URL / model in `providers.<id>`;
//! * **the user's own providers** (`user_providers`): any number of HTTP endpoints, each with its
//!   own id, name, API type, URL, key, model list and extra headers: two OpenAI accounts, three
//!   local servers… They start from a template (an API preset, or a blank OpenAI-compatible
//!   endpoint) with `ai.provider_add`.
//!
//! Keys are stored as plain JSON (the user chose no encryption, see docs/plan.md "Секрети"):
//!
//! ```json
//! { "providers": {
//!     "anthropic": { "api_key": "sk-ant-…", "model": "claude-opus-5-5" },
//!     "ollama": { "model": "qwen3:8b", "models": ["qwen3:8b"] },
//!     "claude-code": { "path": "C:\\Users\\me\\.local\\bin\\claude.exe" }
//!   },
//!   "user_providers": [
//!     { "id": "openai-work", "name": "OpenAI (work)", "api": "openai", "template": "openai",
//!       "api_key": "sk-…", "base_url": "https://api.openai.com/v1" },
//!     { "id": "home-server", "name": "Home server", "api": "openai",
//!       "base_url": "http://10.0.0.5:8000/v1", "headers": { "X-Team": "lab" } }
//!   ],
//!   "default_provider": "openai-work" }
//! ```
//!
//! Older files had one built-in `custom` endpoint in `providers.custom`; [`AiConfig::load`] moves
//! it into `user_providers` under the same id, so sessions and `default_provider` keep working.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bmcp_core::{CmdError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::model::{Api, Client};

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
    /// Default API root (empty for the CLIs).
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

/// Every built-in provider, in the order the UI lists them.
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
];

/// The template name of a blank OpenAI-compatible endpoint, and the id the single "custom"
/// provider of older versions keeps after the migration.
pub const BLANK_TEMPLATE: &str = "custom";

/// Name given to the migrated "custom" provider (the user can rename it).
const LEGACY_CUSTOM_NAME: &str = "Custom (OpenAI-compatible)";

/// Longest display name / id of a user provider.
const MAX_NAME: usize = 64;
const MAX_ID: usize = 48;

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// What the user changed for one provider. Unset fields fall back to the preset (built-ins) or
/// the template (user providers).
///
/// Other parts of the crate may add optional fields here (e.g. CLI permission settings); every
/// field must have a serde default so old files keep loading.
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
    /// API providers: extra HTTP headers sent with every request (OpenRouter's `HTTP-Referer`,
    /// `OpenAI-Organization`, a proxy's routing header…). Not treated as secret.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// One provider the user added (`ai.provider_add`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserProvider {
    /// Unique among all providers; never one of the [`PRESETS`] ids.
    pub id: String,
    pub name: String,
    /// Wire protocol of the endpoint.
    pub api: Api,
    /// The preset it was made from (`openai`, `ollama`…); `None` = a blank endpoint. Decides
    /// whether a key is required and gives the suggested models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(flatten)]
    pub settings: ProviderSettings,
}

/// A new user provider (`ai.provider_add`); see [`AiConfig::add_provider`].
#[derive(Debug, Clone, Default)]
pub struct NewProvider {
    pub name: String,
    /// An API preset id, or `None` / [`BLANK_TEMPLATE`] for a blank endpoint.
    pub template: Option<String>,
    /// `None` = made from the name.
    pub id: Option<String>,
    /// `None` = the template's API (OpenAI-compatible for a blank one).
    pub api: Option<Api>,
    /// `None` = the template's URL (required for a blank one).
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub headers: BTreeMap<String, String>,
}

/// Changes to one provider (`ai.provider_set`): `None` keeps a field, `Some("")` clears it.
#[derive(Debug, Clone, Default)]
pub struct ProviderChanges {
    /// User providers only.
    pub name: Option<String>,
    /// User providers only.
    pub api: Option<Api>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// CLI providers only.
    pub path: Option<String>,
    /// API providers only; replaces every extra header (`{}` removes them all).
    pub headers: Option<BTreeMap<String, String>>,
    /// `true` = default of background AI features; `false` = stop being it.
    pub default: Option<bool>,
}

/// The whole `ai.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AiConfig {
    /// Overrides of the built-in presets, by preset id.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderSettings>,
    /// The user's own providers, in the order they were added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_providers: Vec<UserProvider>,
    /// Provider used by background AI features (self-healing, `page.extract`) when the call
    /// names none. `None` = the first configured one (see `AiService::default_provider`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
}

impl AiConfig {
    /// Read `path`; a missing or broken file gives the defaults (a broken one is logged).
    /// Settings of older versions are migrated in memory (saved with the next change).
    pub fn load(path: &Path) -> Self {
        let mut config = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(%err, path = %path.display(), "ignoring unreadable ai.json");
                AiConfig::default()
            }),
            Err(_) => AiConfig::default(),
        };
        if config.migrate() {
            tracing::info!("moved the old 'custom' AI provider into user_providers");
        }
        config
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

    /// The old single `providers.custom` endpoint becomes the user provider `custom` (same id,
    /// so sessions, scripts and `default_provider` naming it keep working). Returns whether
    /// anything changed.
    pub fn migrate(&mut self) -> bool {
        let Some(old) = self.providers.remove(BLANK_TEMPLATE) else {
            return false;
        };
        let used = old != ProviderSettings::default()
            || self.default_provider.as_deref() == Some(BLANK_TEMPLATE);
        if used && self.user(BLANK_TEMPLATE).is_none() {
            self.user_providers.push(UserProvider {
                id: BLANK_TEMPLATE.into(),
                name: self.free_name(LEGACY_CUSTOM_NAME),
                api: Api::OpenAi,
                template: None,
                settings: old,
            });
        }
        true
    }

    /// Stored settings of a built-in or user provider (defaults when there are none).
    pub fn settings(&self, id: &str) -> ProviderSettings {
        match self.user(id) {
            Some(user) => user.settings.clone(),
            None => self.providers.get(id).cloned().unwrap_or_default(),
        }
    }

    /// Mutable settings of a known provider (`None` for an unknown id).
    pub fn settings_mut(&mut self, id: &str) -> Option<&mut ProviderSettings> {
        if preset(id).is_some() {
            return Some(self.providers.entry(id.to_string()).or_default());
        }
        self.user_mut(id).map(|u| &mut u.settings)
    }

    pub fn user(&self, id: &str) -> Option<&UserProvider> {
        self.user_providers.iter().find(|u| u.id == id)
    }

    pub fn user_mut(&mut self, id: &str) -> Option<&mut UserProvider> {
        self.user_providers.iter_mut().find(|u| u.id == id)
    }

    /// Every provider id: the built-ins in [`PRESETS`] order, then the user's in added order.
    pub fn ids(&self) -> Vec<String> {
        PRESETS
            .iter()
            .map(|p| p.id.to_string())
            .chain(self.user_providers.iter().map(|u| u.id.clone()))
            .collect()
    }

    /// A provider merged with its settings, or `NOT_FOUND` listing the known ids.
    pub fn resolve(&self, id: &str) -> Result<Resolved, CmdError> {
        if let Some(preset) = preset(id) {
            return Ok(Resolved::builtin(preset, self.settings(id)));
        }
        if let Some(user) = self.user(id) {
            return Ok(Resolved::user(user));
        }
        Err(self.unknown(id))
    }

    /// `NOT_FOUND` for `id`, listing the known ids.
    fn unknown(&self, id: &str) -> CmdError {
        let ids = self
            .ids()
            .into_iter()
            .map(serde_json::Value::from)
            .collect();
        CmdError::new(ErrorCode::NotFound, format!("unknown AI provider '{id}'"))
            .with_hint("Call ai_providers to list the providers, ai_provider_add to add one.")
            .with_candidates(ids)
    }

    /// Add a user provider; returns its id. Nothing changes on error.
    pub fn add_provider(&mut self, new: NewProvider) -> Result<String, CmdError> {
        let name = self.check_name(&new.name, None)?;
        let template = match new.template.as_deref().map(str::trim) {
            None | Some("") | Some(BLANK_TEMPLATE) => None,
            Some(id) => Some(api_preset(id)?),
        };
        let api = new
            .api
            .or(template.and_then(preset_api))
            .unwrap_or(Api::OpenAi);
        let base_url = match new.base_url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => check_base_url(url)?,
            _ => match template {
                Some(t) => t.base_url.to_string(),
                None => {
                    return Err(CmdError::invalid_params(
                        "base_url is required for a blank provider",
                    )
                    .with_hint(
                        "Pass the API root, e.g. \"http://localhost:8000/v1\", or start from a \
                         template such as \"ollama\" or \"openai\".",
                    ));
                }
            },
        };
        let id = match new.id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => self.check_new_id(id)?,
            _ => self.id_from_name(&name),
        };
        self.user_providers.push(UserProvider {
            id: id.clone(),
            name,
            api,
            template: template.map(|t| t.id.to_string()),
            settings: ProviderSettings {
                api_key: non_empty(new.api_key),
                base_url: Some(base_url),
                model: non_empty(new.model),
                headers: check_headers(&new.headers)?,
                ..Default::default()
            },
        });
        Ok(id)
    }

    /// Remove a user provider. Returns whether it was the default of background AI (which is
    /// automatic again afterwards).
    pub fn remove_provider(&mut self, id: &str) -> Result<bool, CmdError> {
        if preset(id).is_some() {
            return Err(CmdError::invalid_params(format!(
                "'{id}' is a built-in provider and cannot be removed"
            ))
            .with_hint("Clear its key instead: ai_provider_set {id, api_key: \"\"}."));
        }
        let Some(index) = self.user_providers.iter().position(|u| u.id == id) else {
            return Err(self.unknown(id));
        };
        self.user_providers.remove(index);
        let was_default = self.default_provider.as_deref() == Some(id);
        if was_default {
            self.default_provider = None;
        }
        Ok(was_default)
    }

    /// Apply `ai.provider_set` changes. Nothing changes on error.
    pub fn update_provider(&mut self, id: &str, changes: ProviderChanges) -> Result<(), CmdError> {
        let mut next = self.clone();
        // Older clients (and scripts) configure the single custom endpoint with
        // `ai.provider_set {id: "custom", base_url}`: create it on first use.
        if id == BLANK_TEMPLATE && next.user(id).is_none() {
            next.user_providers.push(UserProvider {
                id: BLANK_TEMPLATE.into(),
                name: next.free_name(LEGACY_CUSTOM_NAME),
                api: Api::OpenAi,
                template: None,
                settings: ProviderSettings::default(),
            });
        }
        next.apply_changes(id, changes)?;
        *self = next;
        Ok(())
    }

    fn apply_changes(&mut self, id: &str, ch: ProviderChanges) -> Result<(), CmdError> {
        let kind = self.resolve(id)?.kind;
        let builtin = preset(id).is_some();
        if builtin && (ch.name.is_some() || ch.api.is_some()) {
            return Err(CmdError::invalid_params(format!(
                "'{id}' is a built-in provider: its name and API type are fixed"
            ))
            .with_hint(
                "Add your own provider with ai_provider_add (it can start from this one).",
            ));
        }
        let is_cli = !matches!(kind, Kind::Api(_));
        if is_cli && ch.headers.as_ref().is_some_and(|h| !h.is_empty()) {
            return Err(CmdError::invalid_params(format!(
                "'{id}' is a CLI agent: extra HTTP headers apply to API providers only"
            )));
        }
        let base_url = match ch.base_url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => Some(check_base_url(url)?),
            other => other.map(String::from),
        };
        let headers = ch.headers.as_ref().map(check_headers).transpose()?;
        if !builtin {
            let name = ch
                .name
                .as_deref()
                .map(|n| self.check_name(n, Some(id)))
                .transpose()?;
            let user = self.user_mut(id).expect("resolved above");
            if let Some(name) = name {
                user.name = name;
            }
            if let Some(api) = ch.api {
                user.api = api;
            }
        }
        let settings = self.settings_mut(id).expect("resolved above");
        set(&mut settings.api_key, ch.api_key);
        set(&mut settings.base_url, base_url);
        set(&mut settings.model, ch.model);
        if is_cli {
            set(&mut settings.path, ch.path);
        }
        if let Some(headers) = headers {
            settings.headers = headers;
        }
        match ch.default {
            Some(true) => self.default_provider = Some(id.to_string()),
            Some(false) if self.default_provider.as_deref() == Some(id) => {
                self.default_provider = None
            }
            _ => {}
        }
        Ok(())
    }

    /// The display names in use, except the provider `except`.
    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let same = |other: &str| other.trim().to_lowercase() == name.trim().to_lowercase();
        PRESETS.iter().any(|p| same(p.name))
            || self
                .user_providers
                .iter()
                .any(|u| Some(u.id.as_str()) != except && same(&u.name))
    }

    /// `name` trimmed, if it is non-empty, short enough and not used by another provider.
    fn check_name(&self, name: &str, except: Option<&str>) -> Result<String, CmdError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CmdError::invalid_params("the provider name is empty"));
        }
        if name.chars().count() > MAX_NAME {
            return Err(CmdError::invalid_params(format!(
                "the provider name is longer than {MAX_NAME} characters"
            )));
        }
        if self.name_taken(name, except) {
            return Err(CmdError::invalid_params(format!(
                "a provider named '{name}' already exists"
            ))
            .with_hint(
                "Pick another name, e.g. add the account or the machine: \"OpenAI (work)\".",
            ));
        }
        Ok(name.to_string())
    }

    /// `base`, or `base 2`, `base 3`… whichever is free.
    fn free_name(&self, base: &str) -> String {
        (1..)
            .map(|n| match n {
                1 => base.to_string(),
                n => format!("{base} {n}"),
            })
            .find(|name| !self.name_taken(name, None))
            .expect("some suffix is free")
    }

    fn id_taken(&self, id: &str) -> bool {
        preset(id).is_some() || self.user(id).is_some()
    }

    fn check_new_id(&self, id: &str) -> Result<String, CmdError> {
        let valid = id.len() <= MAX_ID
            && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c));
        if !valid {
            return Err(CmdError::invalid_params(format!(
                "bad provider id '{id}': use lowercase letters, digits, '-', '_' or '.' \
                 (at most {MAX_ID})"
            )));
        }
        if self.id_taken(id) {
            return Err(CmdError::invalid_params(format!(
                "a provider with id '{id}' already exists"
            ))
            .with_hint("Leave `id` out to get a free one made from the name."));
        }
        Ok(id.to_string())
    }

    /// A free id made from the display name: "OpenAI (work)" → `openai-work`.
    fn id_from_name(&self, name: &str) -> String {
        let base = slug(name);
        (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            })
            .find(|id| !self.id_taken(id))
            .expect("some suffix is free")
    }
}

/// Lowercase ASCII letters and digits joined by `-`; `provider` when nothing is left (a name
/// in another script, e.g. Ukrainian).
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_end_matches('-').chars().take(MAX_ID - 4).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "provider".into()
    } else {
        out
    }
}

/// `Some("")` clears a field, `None` keeps it.
fn set(slot: &mut Option<String>, value: Option<String>) {
    if let Some(value) = value {
        *slot = Some(value.trim().to_string()).filter(|v| !v.is_empty());
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn preset_api(preset: &Preset) -> Option<Api> {
    match preset.kind {
        Kind::Api(api) => Some(api),
        _ => None,
    }
}

/// A preset a user provider can start from: any HTTP API preset (not the CLIs).
fn api_preset(id: &str) -> Result<&'static Preset, CmdError> {
    let templates = || {
        PRESETS
            .iter()
            .filter(|p| preset_api(p).is_some())
            .map(|p| serde_json::Value::from(p.id))
            .chain([serde_json::Value::from(BLANK_TEMPLATE)])
            .collect()
    };
    match preset(id) {
        Some(p) if preset_api(p).is_some() => Ok(p),
        Some(p) => Err(CmdError::invalid_params(format!(
            "{} is a CLI agent and cannot be a template",
            p.name
        ))
        .with_candidates(templates())),
        None => Err(
            CmdError::new(ErrorCode::NotFound, format!("unknown template '{id}'"))
                .with_hint(
                    "Use an API provider id from ai_providers, or \"custom\" for a blank one.",
                )
                .with_candidates(templates()),
        ),
    }
}

/// A usable API root: an absolute `http(s)://host…` URL, without a trailing `/`.
pub fn check_base_url(url: &str) -> Result<String, CmdError> {
    let url = url.trim();
    let bad = |why: &str| {
        CmdError::invalid_params(format!("bad base URL '{url}': {why}"))
            .with_hint("Use the API root, e.g. \"https://api.openai.com/v1\" or \"http://localhost:11434/v1\".")
    };
    let parsed = reqwest::Url::parse(url).map_err(|e| bad(&e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bad("it must start with http:// or https://"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(bad("it has no host"));
    }
    Ok(url.trim_end_matches('/').to_string())
}

/// Trimmed headers if every name and value is valid in HTTP; empty names are dropped.
pub fn check_headers(
    headers: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, CmdError> {
    let mut out = BTreeMap::new();
    for (name, value) in headers {
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() {
            continue;
        }
        crate::model::header_pair(name, value)
            .map_err(|why| CmdError::invalid_params(format!("bad header '{name}': {why}")))?;
        out.insert(name.to_string(), value.to_string());
    }
    Ok(out)
}

/// Why a provider cannot be used yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    NoBaseUrl,
    NoApiKey,
    /// The Claude Code / Codex binary was not found.
    NotInstalled,
}

/// A provider merged with the user's settings: what a session actually uses.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// `false` for the user's own providers.
    pub builtin: bool,
    /// The built-in preset, or the template a user provider was made from.
    pub preset: Option<&'static Preset>,
    pub needs_key: bool,
    pub base_url: String,
    pub api_key: Option<String>,
    /// Configured model, else the preset default, else the first known model.
    pub model: Option<String>,
    pub models: Vec<String>,
    /// Extra HTTP headers (API providers).
    pub headers: BTreeMap<String, String>,
    /// CLI providers: the binary, when found.
    pub binary: Option<PathBuf>,
    /// The stored settings as they are, for options not resolved above.
    pub settings: ProviderSettings,
}

impl Resolved {
    pub fn builtin(preset: &'static Preset, settings: ProviderSettings) -> Self {
        let binary = match preset.kind {
            Kind::ClaudeCode => crate::cli::find_claude(settings.path.as_deref()),
            Kind::Codex => crate::cli::find_codex(settings.path.as_deref()),
            Kind::Api(_) => None,
        };
        let base_url = settings
            .base_url
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| preset.base_url.to_string());
        Self::merge(
            preset.id,
            preset.name,
            preset.kind,
            Some(preset),
            settings,
            base_url,
            binary,
        )
    }

    pub fn user(user: &UserProvider) -> Self {
        // The template's defaults only make sense while the API type is still the template's.
        let template = user
            .template
            .as_deref()
            .and_then(preset)
            .filter(|t| t.kind == Kind::Api(user.api));
        let mut resolved = Self::merge(
            &user.id,
            &user.name,
            Kind::Api(user.api),
            template,
            user.settings.clone(),
            user.settings.base_url.clone().unwrap_or_default(),
            None,
        );
        resolved.builtin = false;
        // Remembered even after an API type change: an OpenAI account still needs its key.
        resolved.needs_key = user
            .template
            .as_deref()
            .and_then(preset)
            .is_some_and(|t| t.needs_key);
        resolved
    }

    fn merge(
        id: &str,
        name: &str,
        kind: Kind,
        preset: Option<&'static Preset>,
        settings: ProviderSettings,
        base_url: String,
        binary: Option<PathBuf>,
    ) -> Self {
        let models = if settings.models.is_empty() {
            preset
                .map(|p| p.models.iter().map(|m| m.to_string()).collect())
                .unwrap_or_default()
        } else {
            settings.models.clone()
        };
        let model = settings
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .or_else(|| {
                preset
                    .map(|p| p.model.to_string())
                    .filter(|m| !m.is_empty())
            })
            .or_else(|| match kind {
                // CLI agents have their own default model.
                Kind::Api(_) => models.first().cloned(),
                _ => None,
            });
        Resolved {
            id: id.to_string(),
            name: name.to_string(),
            kind,
            builtin: true,
            preset,
            needs_key: preset.is_some_and(|p| p.needs_key),
            base_url,
            api_key: settings.api_key.clone().filter(|k| !k.is_empty()),
            model,
            models,
            headers: settings.headers.clone(),
            binary,
            settings,
        }
    }

    /// What is missing before the provider can be used, if anything.
    pub fn problem(&self) -> Option<Problem> {
        match self.kind {
            Kind::Api(_) if self.base_url.is_empty() => Some(Problem::NoBaseUrl),
            Kind::Api(_) if self.needs_key && self.api_key.is_none() => Some(Problem::NoApiKey),
            Kind::Api(_) => None,
            Kind::ClaudeCode | Kind::Codex => {
                self.binary.is_none().then_some(Problem::NotInstalled)
            }
        }
    }

    /// Ready to use: API providers have a URL (and a key when required); CLIs are installed.
    pub fn configured(&self) -> bool {
        self.problem().is_none()
    }

    /// `INVALID_PARAMS` saying what is missing, with how to fix it.
    pub fn ensure_configured(&self) -> Result<(), CmdError> {
        let Some(problem) = self.problem() else {
            return Ok(());
        };
        let id = &self.id;
        let (message, hint) = match problem {
            Problem::NoBaseUrl => (
                format!("AI provider '{id}' has no base URL"),
                "Set one with ai_provider_set {id, base_url}.",
            ),
            Problem::NoApiKey => (
                format!("AI provider '{id}' needs an API key"),
                "Set it with ai_provider_set {id, api_key}.",
            ),
            Problem::NotInstalled if self.kind == Kind::ClaudeCode => (
                format!("{} is not installed", self.name),
                "Install Claude Code (`claude` on PATH) or set its path with ai_provider_set.",
            ),
            Problem::NotInstalled => (
                format!("{} is not installed", self.name),
                "Install Codex (`codex` on PATH) or set its path with ai_provider_set.",
            ),
        };
        Err(CmdError::invalid_params(message).with_hint(hint))
    }

    /// HTTP client of an API provider (with its key and extra headers); `None` for the CLIs.
    pub fn client(&self) -> Option<Client> {
        match self.kind {
            Kind::Api(api) => Some(
                Client::new(api, self.base_url.clone(), self.api_key.clone())
                    .with_headers(&self.headers),
            ),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
