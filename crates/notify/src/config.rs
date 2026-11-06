//! `data/config/notify.json`: which channels are on, and which events go where.
//!
//! Plain JSON (no encryption, by the user's choice). Every field has a default, so a partial
//! file loads fine; a missing file is created with the defaults on first start.
//!
//! ```json
//! {
//!   "channels": {
//!     "ui": true,
//!     "os": true,
//!     "telegram": { "enabled": false, "bot_token": "", "chat_id": "" },
//!     "webhook":  { "enabled": false, "url": "" }
//!   },
//!   "rules": [
//!     { "topic": "run.finished", "channels": ["ui", "os", "telegram", "webhook"] },
//!     { "topic": "run.heal_proposed", "channels": ["ui", "os", "telegram", "webhook"] },
//!     { "topic": "ai.done", "channels": ["os", "telegram"], "when": "unfocused" }
//!   ],
//!   "limits": { "dedup_secs": 10, "aggregate_secs": 60, "max_per_minute": 20 }
//! }
//! ```

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a notification can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// A toast in the browser UI (`ui.notification` event).
    Ui,
    /// A Windows toast notification.
    Os,
    /// A Telegram bot message (with answer buttons for questions).
    Telegram,
    /// `POST <url>` with the notification as JSON.
    Webhook,
}

impl Channel {
    pub const ALL: [Channel; 4] = [
        Channel::Ui,
        Channel::Os,
        Channel::Telegram,
        Channel::Webhook,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Ui => "ui",
            Channel::Os => "os",
            Channel::Telegram => "telegram",
            Channel::Webhook => "webhook",
        }
    }
}

/// The whole configuration file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NotifyConfig {
    pub channels: Channels,
    /// Event topic → channels. The first rule whose topic matches and whose `when` holds wins.
    pub rules: Vec<Rule>,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Channels {
    /// Toasts in the browser UI.
    pub ui: bool,
    /// Windows toast notifications.
    pub os: bool,
    pub telegram: TelegramConfig,
    pub webhook: WebhookConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    /// Token from @BotFather (`123456:ABC…`). Stored as plain text.
    pub bot_token: String,
    /// Your chat with the bot (a number like `"123456789"`), or `"@channel"`. Button presses
    /// from any other chat are ignored.
    pub chat_id: String,
    /// Bot API base URL; only changed for tests or a self-hosted Bot API server.
    pub api_base: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WebhookConfig {
    pub enabled: bool,
    /// Receives `POST` with `{id, title, body, level, topic, data, actions, ts_ms}`.
    pub url: String,
}

/// One routing rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    /// Exact topic (`run.finished`), a prefix pattern (`run.*`) or `*`.
    pub topic: String,
    /// Where matching events go (only the channels that are enabled are used).
    pub channels: Vec<Channel>,
    /// Extra condition; default `always`.
    #[serde(default)]
    pub when: When,
    /// Collect bursts of this event (per run / session) into one summary per
    /// `limits.aggregate_secs`: the first one is sent at once, the rest are counted.
    #[serde(default)]
    pub aggregate: bool,
}

/// Condition of a [`Rule`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum When {
    #[default]
    Always,
    /// Only when the browser window is not the foreground window.
    Unfocused,
    /// Only when the event reports a failure (`ok: false`, `status: "failed"` or `failed > 0`).
    Failed,
}

/// Anti-spam knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Limits {
    /// The same title + text again within this many seconds is dropped.
    pub dedup_secs: u64,
    /// Window of `aggregate` rules.
    pub aggregate_secs: u64,
    /// Per channel; extra notifications are dropped and counted ("+N suppressed").
    /// Questions (with buttons) are never dropped.
    pub max_per_minute: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            dedup_secs: 10,
            aggregate_secs: 60,
            max_per_minute: 20,
        }
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        TelegramConfig {
            enabled: false,
            bot_token: String::new(),
            chat_id: String::new(),
            api_base: "https://api.telegram.org".into(),
        }
    }
}

impl Default for Channels {
    fn default() -> Self {
        Channels {
            ui: true,
            os: true,
            telegram: TelegramConfig::default(),
            webhook: WebhookConfig::default(),
        }
    }
}

impl Default for NotifyConfig {
    fn default() -> Self {
        use Channel::*;
        let rule = |topic: &str, channels: &[Channel], when: When, aggregate: bool| Rule {
            topic: topic.into(),
            channels: channels.to_vec(),
            when,
            aggregate,
        };
        NotifyConfig {
            channels: Channels::default(),
            rules: vec![
                rule(
                    "run.finished",
                    &[Ui, Os, Telegram, Webhook],
                    When::Always,
                    false,
                ),
                rule(
                    "run.heal_needed",
                    &[Ui, Os, Telegram, Webhook],
                    When::Always,
                    false,
                ),
                // The AI's patch for it (Accept / Reject), and how that ended.
                rule(
                    "run.heal_proposed",
                    &[Ui, Os, Telegram, Webhook],
                    When::Always,
                    false,
                ),
                rule("run.heal_applied", &[Ui, Telegram], When::Always, false),
                rule(
                    "run.heal_failed",
                    &[Ui, Os, Telegram, Webhook],
                    When::Always,
                    false,
                ),
                // Row failures come in bursts: one message per run per minute at most.
                rule("run.row_failed", &[Ui], When::Always, true),
                rule("ai.done", &[Os, Telegram], When::Unfocused, false),
                rule("download.done", &[Ui, Os], When::Always, false),
                rule(
                    "trigger.fired",
                    &[Ui, Os, Telegram, Webhook],
                    When::Failed,
                    false,
                ),
            ],
            limits: Limits::default(),
        }
    }
}

impl TelegramConfig {
    /// Enabled and filled in.
    pub fn usable(&self) -> bool {
        self.enabled && self.is_configured()
    }

    /// Token and chat set (enabled or not).
    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty() && !self.chat_id.trim().is_empty()
    }
}

impl NotifyConfig {
    /// Read `path`. Missing file → defaults (and the file is written so users can edit it).
    /// Broken JSON → defaults, logged, the file is left alone.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(config) => config,
                Err(err) => {
                    tracing::warn!(%err, path = %path.display(), "bad notify.json; using defaults");
                    NotifyConfig::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let config = NotifyConfig::default();
                if let Err(err) = config.save(path) {
                    tracing::warn!(%err, "cannot write default notify.json");
                }
                config
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "cannot read notify.json");
                NotifyConfig::default()
            }
        }
    }

    /// Write pretty JSON (to a temp file first, so a crash never leaves half a file).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Is `channel` switched on?
    pub fn enabled(&self, channel: Channel) -> bool {
        match channel {
            Channel::Ui => self.channels.ui,
            Channel::Os => self.channels.os,
            Channel::Telegram => self.channels.telegram.usable(),
            Channel::Webhook => {
                self.channels.webhook.enabled && !self.channels.webhook.url.trim().is_empty()
            }
        }
    }

    /// A copy safe to show (to the UI or an AI agent): the bot token is masked.
    pub fn masked(&self) -> Self {
        let mut copy = self.clone();
        copy.channels.telegram.bot_token = mask(&self.channels.telegram.bot_token);
        copy
    }
}

/// `"123456:ABCDEFwxyz"` → `"…wxyz"`; empty stays empty.
pub fn mask(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

/// Is this the masked form [`mask`] produces (so `config_set` should keep the stored token)?
pub fn is_masked(token: &str) -> bool {
    token.starts_with('…')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("notify.json");
        // Missing file → defaults, and the file is created.
        let first = NotifyConfig::load(&path);
        assert_eq!(first, NotifyConfig::default());
        assert!(path.exists());

        let mut changed = first.clone();
        changed.channels.telegram = TelegramConfig {
            enabled: true,
            bot_token: "123:abc".into(),
            chat_id: "42".into(),
            api_base: "http://127.0.0.1:1".into(),
        };
        changed.channels.os = false;
        changed.rules.push(Rule {
            topic: "net.*".into(),
            channels: vec![Channel::Webhook],
            when: When::Failed,
            aggregate: true,
        });
        changed.limits.max_per_minute = 5;
        changed.save(&path).unwrap();
        assert_eq!(NotifyConfig::load(&path), changed);
    }

    #[test]
    fn partial_files_fill_in_defaults() {
        let config: NotifyConfig =
            serde_json::from_str(r#"{"channels": {"os": false, "telegram": {"chat_id": "7"}}}"#)
                .unwrap();
        assert!(config.channels.ui);
        assert!(!config.channels.os);
        assert_eq!(config.channels.telegram.chat_id, "7");
        assert_eq!(
            config.channels.telegram.api_base,
            "https://api.telegram.org"
        );
        assert_eq!(config.rules, NotifyConfig::default().rules);
    }

    #[test]
    fn broken_file_gives_defaults_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify.json");
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(NotifyConfig::load(&path), NotifyConfig::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ nope");
    }

    #[test]
    fn channels_need_config_to_count_as_enabled() {
        let mut config = NotifyConfig::default();
        assert!(config.enabled(Channel::Ui) && config.enabled(Channel::Os));
        config.channels.telegram.enabled = true;
        assert!(!config.enabled(Channel::Telegram), "no token yet");
        config.channels.telegram.bot_token = "1:x".into();
        config.channels.telegram.chat_id = "1".into();
        assert!(config.enabled(Channel::Telegram));
        config.channels.webhook.enabled = true;
        assert!(!config.enabled(Channel::Webhook), "no url yet");
    }

    #[test]
    fn token_is_masked() {
        assert_eq!(mask("123456:ABCDEFwxyz"), "…wxyz");
        assert_eq!(mask(""), "");
        assert!(is_masked(&mask("abc")));
        let mut config = NotifyConfig::default();
        config.channels.telegram.bot_token = "123456:secret".into();
        assert_eq!(config.masked().channels.telegram.bot_token, "…cret");
    }
}
