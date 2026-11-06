//! What a notification is, independent of the channel that shows it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How important a notification is (UI color, Telegram prefix, toast sound).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Success => "success",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }
}

/// A button on a notification. Pressing it answers the notification (see `ui.answer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Action {
    /// Short id sent back when pressed (`skip`, `c0`, …). At most 16 bytes, so it fits
    /// Telegram's 64-byte callback data together with the notification id.
    pub id: String,
    /// Button text.
    pub label: String,
    /// Only shown on remote channels (Telegram), e.g. "Open in browser".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remote_only: bool,
}

impl Action {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Action {
            id: id.into(),
            label: label.into(),
            remote_only: false,
        }
    }

    pub fn remote_only(mut self) -> Self {
        self.remote_only = true;
        self
    }
}

/// One notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    /// `n1`, `n2`, … (unique per browser run).
    pub id: String,
    pub title: String,
    pub body: String,
    pub level: Level,
    /// Buttons; a notification with buttons is a question and waits for an answer.
    pub actions: Vec<Action>,
    /// The event topic it came from (`run.finished`), if any.
    pub topic: Option<String>,
    /// The event payload (sent to webhooks).
    pub data: Value,
    /// Aggregation group (`run.row_failed:r12`) and its label ("Run r12 row failures").
    pub group: Option<(String, String)>,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>, level: Level) -> Self {
        Notification {
            id: String::new(),
            title: title.into(),
            body: body.into(),
            level,
            actions: Vec::new(),
            topic: None,
            data: Value::Null,
            group: None,
        }
    }

    pub fn is_question(&self) -> bool {
        !self.actions.is_empty()
    }

    /// Payload of the `ui.notification` event (what the UI toast host renders).
    pub fn ui_payload(&self) -> Value {
        let actions: Vec<&Action> = self.actions.iter().filter(|a| !a.remote_only).collect();
        json!({
            "id": self.id,
            "title": self.title,
            "body": self.body,
            "level": self.level,
            "actions": actions,
            // Questions stay until answered; plain notifications fade out.
            "sticky": !actions.is_empty(),
            "topic": self.topic,
        })
    }

    /// Body of the webhook POST.
    pub fn webhook_payload(&self, ts_ms: u64) -> Value {
        json!({
            "source": "browsermcp",
            "id": self.id,
            "title": self.title,
            "body": self.body,
            "level": self.level,
            "topic": self.topic,
            "data": self.data,
            "actions": self.actions,
            "ts_ms": ts_ms,
        })
    }

    /// Plain text for Telegram (no Markdown: event text may contain `_*[`).
    pub fn telegram_text(&self) -> String {
        let prefix = match self.level {
            Level::Info => "",
            Level::Success => "[ok] ",
            Level::Warning => "[warning] ",
            Level::Error => "[error] ",
        };
        let text = if self.body.is_empty() {
            format!("{prefix}{}", self.title)
        } else {
            format!("{prefix}{}\n{}", self.title, self.body)
        };
        // Telegram's limit is 4096 characters.
        truncate(&text, 3900)
    }
}

/// Cut `text` to at most `max` characters, ending with `…` when cut.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}
