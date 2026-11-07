//! The models a provider offers, for the model pickers (`ai.models`):
//!
//! * Claude Code: what `claude --model` accepts — the aliases (`opus`, `sonnet`, `haiku`,
//!   `fable`, which follow the latest model of their family) and the full ids of the current
//!   family, plus `default` (Claude Code's own choice);
//! * Codex: `model/list` over `codex app-server` (no thread, no model call), else a short
//!   static list;
//! * API providers: the endpoint's live list (`GET /models`, no tokens spent), else the
//!   configured / suggested models.

use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;

use crate::config::{Kind, Resolved};

/// One entry of `ai.models`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ModelChoice {
    /// What ai.session_start / ai.session_set_model take ("default" = the provider's or
    /// CLI's own default).
    pub id: String,
    /// Display name.
    pub name: String,
    /// A short note (alias of…, description from the provider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ModelChoice {
    fn new(id: &str, name: &str, hint: Option<&str>) -> Self {
        ModelChoice {
            id: id.to_string(),
            name: name.to_string(),
            hint: hint.map(str::to_string),
        }
    }
}

/// `model` values that mean "no model of the chat's own": the provider's default.
pub fn is_default(model: &str) -> bool {
    let model = model.trim();
    model.is_empty() || model.eq_ignore_ascii_case("default")
}

/// Claude Code's choices (`claude --help`: "an alias for the latest model (e.g. 'fable',
/// 'opus', or 'sonnet') or a model's full name").
pub fn claude_models() -> Vec<ModelChoice> {
    vec![
        ModelChoice::new("default", "Default", Some("Claude Code's own choice")),
        ModelChoice::new("opus", "Opus", Some("alias: the latest Opus")),
        ModelChoice::new("sonnet", "Sonnet", Some("alias: the latest Sonnet")),
        ModelChoice::new("haiku", "Haiku", Some("alias: the latest Haiku")),
        ModelChoice::new("fable", "Fable", Some("alias: the latest Fable")),
        ModelChoice::new("claude-opus-5-5", "Claude Opus 5.5", None),
        ModelChoice::new("claude-sonnet-5-5", "Claude Sonnet 5.5", None),
        ModelChoice::new("claude-haiku-5-5", "Claude Haiku 5.5", None),
        ModelChoice::new("claude-fable-5-1", "Claude Fable 5.1", None),
    ]
}

/// Used when Codex does not answer `model/list` (an older Codex, or none installed).
pub fn codex_static_models() -> Vec<ModelChoice> {
    let note = Some("static list: Codex did not list its models");
    vec![
        ModelChoice::new("default", "Default", Some("Codex's own choice")),
        ModelChoice::new("gpt-5.1-codex", "GPT-5.1 Codex", note),
        ModelChoice::new("gpt-5.1-codex-mini", "GPT-5.1 Codex Mini", note),
        ModelChoice::new("gpt-5.1", "GPT-5.1", note),
    ]
}

/// Codex `model/list` result → choices and the default.
pub fn codex_models(result: &serde_json::Value) -> (Vec<ModelChoice>, Option<String>) {
    let mut default = None;
    let mut list = vec![ModelChoice::new(
        "default",
        "Default",
        Some("Codex's own choice"),
    )];
    for (id, name, hint, is_default) in crate::cli::codex::parse_models(result) {
        if is_default {
            default = Some(id.clone());
        }
        list.push(ModelChoice { id, name, hint });
    }
    (list, default)
}

/// API providers: `models` as choices, the default first if it is not among them.
pub fn api_models(models: &[String], default: Option<&str>) -> Vec<ModelChoice> {
    let mut list: Vec<ModelChoice> = Vec::new();
    if let Some(default) = default.filter(|d| !models.iter().any(|m| m == d)) {
        list.push(ModelChoice::new(default, default, None));
    }
    list.extend(models.iter().map(|m| ModelChoice::new(m, m, None)));
    list
}

/// The choices for provider `r` and its default model.
pub async fn list(r: &Resolved, workspace: &std::path::Path) -> (Vec<ModelChoice>, Option<String>) {
    match r.kind {
        Kind::ClaudeCode => (
            claude_models(),
            Some(r.model.clone().unwrap_or_else(|| "default".into())),
        ),
        Kind::Codex => {
            let live = match &r.binary {
                Some(binary) => {
                    let _ = std::fs::create_dir_all(workspace);
                    crate::cli::codex::list_models(binary, workspace).await
                }
                None => Err("codex not found".into()),
            };
            match live {
                Ok(result) => {
                    let (list, default) = codex_models(&result);
                    if list.len() > 1 {
                        return (list, r.model.clone().or(default));
                    }
                    (codex_static_models(), r.model.clone())
                }
                Err(err) => {
                    tracing::debug!(%err, "codex model/list failed; using the static list");
                    (codex_static_models(), r.model.clone())
                }
            }
        }
        Kind::Api(_) => {
            let mut models = r.models.clone();
            if r.problem().is_none()
                && let Some(client) = r.client()
                && let Ok(Ok(live)) =
                    tokio::time::timeout(Duration::from_secs(10), client.list_models()).await
                && !live.is_empty()
            {
                models = live;
            }
            (api_models(&models, r.model.as_deref()), r.model.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_lists() {
        assert!(is_default("") && is_default(" Default ") && !is_default("opus"));
        let claude: Vec<String> = claude_models().into_iter().map(|m| m.id).collect();
        for id in [
            "default",
            "opus",
            "sonnet",
            "haiku",
            "fable",
            "claude-opus-5-5",
            "claude-fable-5-1",
        ] {
            assert!(claude.contains(&id.to_string()), "{id}");
        }
        let (list, default) = codex_models(&json!({ "data": [
            { "id": "a", "model": "gpt-x", "displayName": "GPT X", "description": "fast", "isDefault": true },
            { "id": "b", "model": "gpt-y", "displayName": "GPT Y", "hidden": true },
            { "model": "gpt-z" },
        ]}));
        assert_eq!(default.as_deref(), Some("gpt-x"));
        assert_eq!(
            list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["default", "gpt-x", "gpt-z"]
        );
        assert_eq!(list[1].hint.as_deref(), Some("fast"));
        assert_eq!(list[2].name, "gpt-z");
        let api = api_models(&["m1".into(), "m2".into()], Some("mine"));
        assert_eq!(api[0].id, "mine");
        assert_eq!(api.len(), 3);
        assert_eq!(api_models(&["m1".into()], Some("m1")).len(), 1);
    }
}
