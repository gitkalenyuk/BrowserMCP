//! The few Telegram Bot API calls we need: `sendMessage` (with an inline keyboard),
//! `getUpdates` (long polling for button presses), `answerCallbackQuery`, `editMessageText`.
//!
//! Every call is `POST {api_base}/bot{token}/{method}` with a JSON body; the answer is
//! `{"ok": true, "result": …}` or `{"ok": false, "description": "…"}`. Error messages never
//! contain the URL (it holds the token).

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::TelegramConfig;
use crate::model::{Action, Notification};

/// One button press (`callback_query`) from `getUpdates`.
#[derive(Debug, Clone, PartialEq)]
pub struct Press {
    pub update_id: i64,
    pub callback_id: String,
    /// Our callback data: `"<notification id>|<action id>"`.
    pub data: String,
    pub chat_id: i64,
    pub chat_username: Option<String>,
    pub message_id: Option<i64>,
    /// Who pressed (first name or @username), for the "answered by" line.
    pub from: String,
}

pub struct Bot<'a> {
    http: &'a reqwest::Client,
    config: &'a TelegramConfig,
}

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    description: String,
}

impl<'a> Bot<'a> {
    pub fn new(http: &'a reqwest::Client, config: &'a TelegramConfig) -> Self {
        Bot { http, config }
    }

    async fn call(&self, method: &str, body: Value, timeout: Duration) -> Result<Value, String> {
        let url = format!(
            "{}/bot{}/{method}",
            self.config.api_base.trim_end_matches('/'),
            self.config.bot_token.trim()
        );
        let response = self
            .http
            .post(url)
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| format!("telegram {method}: {}", e.without_url()))?;
        let status = response.status();
        let reply: Reply = response
            .json()
            .await
            .map_err(|e| format!("telegram {method}: HTTP {status}: {}", e.without_url()))?;
        if !reply.ok {
            return Err(format!(
                "telegram {method}: {}",
                if reply.description.is_empty() {
                    format!("HTTP {status}")
                } else {
                    reply.description
                }
            ));
        }
        Ok(reply.result)
    }

    /// Send `n` (with its buttons, one row) and return the message id.
    pub async fn send(&self, n: &Notification) -> Result<i64, String> {
        let result = self
            .call(
                "sendMessage",
                send_body(&self.config.chat_id, n),
                Duration::from_secs(15),
            )
            .await?;
        Ok(result
            .get("message_id")
            .and_then(Value::as_i64)
            .unwrap_or(0))
    }

    /// Plain text message (e.g. the proposed patch), optionally as a reply.
    pub async fn send_text(&self, text: &str, reply_to: Option<i64>) -> Result<i64, String> {
        let mut body = json!({ "chat_id": chat_value(&self.config.chat_id), "text": text });
        if let Some(id) = reply_to {
            body["reply_parameters"] = json!({ "message_id": id });
        }
        let result = self
            .call("sendMessage", body, Duration::from_secs(15))
            .await?;
        Ok(result
            .get("message_id")
            .and_then(Value::as_i64)
            .unwrap_or(0))
    }

    /// Long poll: waits up to `timeout_secs` for updates after `offset`; returns the button
    /// presses (other updates are skipped) and the next offset.
    pub async fn presses(
        &self,
        offset: i64,
        timeout_secs: u64,
    ) -> Result<(Vec<Press>, i64), String> {
        let body = json!({
            "offset": offset,
            "timeout": timeout_secs,
            "allowed_updates": ["callback_query"],
        });
        let result = self
            .call("getUpdates", body, Duration::from_secs(timeout_secs + 10))
            .await?;
        Ok(parse_updates(&result, offset))
    }

    /// Stop the spinner on the pressed button, with a short notice.
    pub async fn answer_press(&self, callback_id: &str, text: &str) -> Result<(), String> {
        let body = json!({ "callback_query_id": callback_id, "text": text });
        self.call("answerCallbackQuery", body, Duration::from_secs(10))
            .await
            .map(drop)
    }

    /// Replace a message's text; with `keep` the buttons stay, without it they go away.
    pub async fn edit(
        &self,
        message_id: i64,
        text: &str,
        keep: Option<&[Action]>,
    ) -> Result<(), String> {
        let mut body = json!({
            "chat_id": chat_value(&self.config.chat_id),
            "message_id": message_id,
            "text": text,
        });
        if let Some(actions) = keep {
            body["reply_markup"] = keyboard(actions);
        }
        self.call("editMessageText", body, Duration::from_secs(10))
            .await
            .map(drop)
    }
}

/// Numeric chat ids go out as numbers, `@channel` names as strings.
fn chat_value(chat_id: &str) -> Value {
    let chat_id = chat_id.trim();
    chat_id
        .parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::from(chat_id))
}

/// `{"inline_keyboard": [[{text, callback_data}, …]]}` with every button in one row
/// (two rows when there are more than three).
pub fn keyboard(actions: &[Action]) -> Value {
    let buttons: Vec<Value> = actions
        .iter()
        .map(|a| json!({ "text": a.label, "callback_data": a.id }))
        .collect();
    let rows: Vec<Value> = buttons
        .chunks(3)
        .map(|row| Value::from(row.to_vec()))
        .collect();
    json!({ "inline_keyboard": rows })
}

/// Body of `sendMessage` for a notification; button data is `"<id>|<action>"`.
pub fn send_body(chat_id: &str, n: &Notification) -> Value {
    let mut body = json!({
        "chat_id": chat_value(chat_id),
        "text": n.telegram_text(),
        "link_preview_options": { "is_disabled": true },
    });
    if n.is_question() {
        let actions: Vec<Action> = n
            .actions
            .iter()
            .map(|a| Action {
                id: callback_data(&n.id, &a.id),
                ..a.clone()
            })
            .collect();
        body["reply_markup"] = keyboard(&actions);
    }
    body
}

pub fn callback_data(notification: &str, action: &str) -> String {
    format!("{notification}|{action}")
}

/// `"n3|skip"` → `("n3", "skip")`.
pub fn parse_callback(data: &str) -> Option<(&str, &str)> {
    data.split_once('|')
}

fn parse_updates(result: &Value, mut offset: i64) -> (Vec<Press>, i64) {
    let mut presses = Vec::new();
    for update in result.as_array().into_iter().flatten() {
        let update_id = update.get("update_id").and_then(Value::as_i64).unwrap_or(0);
        offset = offset.max(update_id + 1);
        let Some(cb) = update.get("callback_query") else {
            continue;
        };
        let message = cb.get("message").unwrap_or(&Value::Null);
        let chat = message.get("chat").unwrap_or(&Value::Null);
        let from = cb.get("from").unwrap_or(&Value::Null);
        let from_name = from
            .get("username")
            .and_then(Value::as_str)
            .map(|u| format!("@{u}"))
            .or_else(|| {
                from.get("first_name")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .unwrap_or_default();
        presses.push(Press {
            update_id,
            callback_id: cb
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            data: cb
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            chat_id: chat.get("id").and_then(Value::as_i64).unwrap_or(0),
            chat_username: chat
                .get("username")
                .and_then(Value::as_str)
                .map(String::from),
            message_id: message.get("message_id").and_then(Value::as_i64),
            from: from_name,
        });
    }
    (presses, offset)
}

/// Did this press come from the configured chat? (Anyone can find a bot and press buttons on
/// a forwarded message; only our chat may answer.)
pub fn from_our_chat(press: &Press, config: &TelegramConfig) -> bool {
    let want = config.chat_id.trim();
    match want.strip_prefix('@') {
        Some(name) => press
            .chat_username
            .as_deref()
            .is_some_and(|u| u.eq_ignore_ascii_case(name)),
        None => want.parse::<i64>().ok() == Some(press.chat_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Level;

    #[test]
    fn send_body_with_buttons() {
        let mut n = Notification::new("Proceed?", "Run r1 is paused", Level::Warning);
        n.id = "n4".into();
        n.actions = vec![Action::new("c0", "Yes"), Action::new("c1", "No")];
        let body = send_body("12345", &n);
        assert_eq!(body["chat_id"], json!(12345));
        assert_eq!(body["text"], "[warning] Proceed?\nRun r1 is paused");
        assert_eq!(
            body["reply_markup"],
            json!({"inline_keyboard": [[
                {"text": "Yes", "callback_data": "n4|c0"},
                {"text": "No", "callback_data": "n4|c1"},
            ]]})
        );
        // Channels by name stay strings; plain notifications have no keyboard.
        let plain = Notification::new("Hi", "", Level::Info);
        let body = send_body("@mychannel", &plain);
        assert_eq!(body["chat_id"], "@mychannel");
        assert_eq!(body["text"], "Hi");
        assert!(body.get("reply_markup").is_none());
    }

    #[test]
    fn many_buttons_wrap_into_rows_of_three() {
        let actions: Vec<Action> = (0..5).map(|i| Action::new(format!("c{i}"), "x")).collect();
        let k = keyboard(&actions);
        let rows = k["inline_keyboard"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].as_array().unwrap().len(), 3);
    }

    #[test]
    fn parses_presses_and_offsets() {
        let result = json!([
            {"update_id": 10, "message": {"text": "hi"}},
            {"update_id": 11, "callback_query": {
                "id": "cb1", "data": "n2|skip",
                "from": {"id": 5, "first_name": "Ann", "username": "ann"},
                "message": {"message_id": 77, "chat": {"id": 5, "type": "private"}}
            }}
        ]);
        let (presses, offset) = parse_updates(&result, 0);
        assert_eq!(offset, 12);
        assert_eq!(presses.len(), 1);
        assert_eq!(presses[0].data, "n2|skip");
        assert_eq!(presses[0].message_id, Some(77));
        assert_eq!(presses[0].from, "@ann");
        assert_eq!(parse_callback(&presses[0].data), Some(("n2", "skip")));

        let mut config = TelegramConfig {
            chat_id: "5".into(),
            ..TelegramConfig::default()
        };
        assert!(from_our_chat(&presses[0], &config));
        config.chat_id = "6".into();
        assert!(!from_our_chat(&presses[0], &config));
    }
}
