//! Event → rule → notification text.

use serde_json::Value;

use crate::config::{Channel, NotifyConfig, Rule, When};
use crate::heal;
use crate::model::{Level, Notification, truncate};

/// `run.finished` matches `run.finished`, `run.*` and `*`.
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix(".*") {
        Some(prefix) => topic
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.')),
        None => pattern == topic,
    }
}

/// Does the event payload say something failed?
pub fn event_failed(data: &Value) -> bool {
    data.get("ok").and_then(Value::as_bool) == Some(false)
        || matches!(
            data.get("status").and_then(Value::as_str),
            Some("failed" | "error")
        )
        || data.get("failed").and_then(Value::as_u64).unwrap_or(0) > 0
}

/// The rule for this event (first match whose condition holds) and the channels to use
/// (the rule's channels that are enabled). `None` = do not notify.
pub fn route<'a>(
    config: &'a NotifyConfig,
    topic: &str,
    data: &Value,
    window_focused: impl Fn() -> bool,
) -> Option<(&'a Rule, Vec<Channel>)> {
    let rule = config.rules.iter().find(|rule| {
        topic_matches(&rule.topic, topic)
            && match rule.when {
                When::Always => true,
                When::Failed => event_failed(data),
                When::Unfocused => !window_focused(),
            }
    })?;
    let mut channels: Vec<Channel> = Vec::new();
    for &channel in &rule.channels {
        if config.enabled(channel) && !channels.contains(&channel) {
            channels.push(channel);
        }
    }
    (!channels.is_empty()).then_some((rule, channels))
}

fn text(data: &Value, key: &str) -> String {
    match data.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn num(data: &Value, key: &str) -> u64 {
    data.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// `{code, message}` error objects (jobs) or plain strings → `"NOT_FOUND: no button"`.
pub fn error_text(error: &Value) -> String {
    match error {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Object(_) => {
            let code = text(error, "code");
            let message = text(error, "message");
            match (code.is_empty(), message.is_empty()) {
                (false, false) => format!("{code}: {message}"),
                (false, true) => code,
                _ => message,
            }
        }
        other => other.to_string(),
    }
}

/// `83_000` → `"1m 23s"`.
fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// Human text for an event. Known topics get a proper sentence; anything else shows the topic
/// and a short JSON of the payload.
pub fn describe(topic: &str, data: &Value) -> Notification {
    let id = text(data, "id");
    let mut n = match topic {
        "run.finished" => {
            let status = text(data, "status");
            let failed = num(data, "failed");
            let level = match status.as_str() {
                "finished" if failed == 0 => Level::Success,
                "finished" | "cancelled" => Level::Warning,
                _ => Level::Error,
            };
            let mut body = format!(
                "{}/{} rows ok, {failed} failed",
                num(data, "ok"),
                num(data, "total")
            );
            for key in ["cancelled", "needs_heal"] {
                if num(data, key) > 0 {
                    body.push_str(&format!(", {} {}", num(data, key), key.replace('_', " ")));
                }
            }
            body.push_str(&format!(" · {}", duration(num(data, "duration_ms"))));
            Notification::new(format!("Run {id} {status}"), body, level)
        }
        "run.heal_needed" => {
            let mut n = Notification::new(
                format!("Run {id} needs a fix"),
                heal::describe(data),
                Level::Warning,
            );
            n.actions = heal::actions(data);
            n
        }
        "run.row_failed" => {
            let mut n = Notification::new(
                format!("Run {id}: row {} failed", text(data, "row")),
                error_text(data.get("error").unwrap_or(&Value::Null)),
                Level::Error,
            );
            n.group = Some((
                format!("run.row_failed:{id}"),
                format!("Run {id} row failures"),
            ));
            n
        }
        "ai.done" => {
            let reason = text(data, "reason");
            let level = if reason.starts_with("error") {
                Level::Error
            } else {
                Level::Success
            };
            Notification::new(
                format!("AI agent {} finished", text(data, "session")),
                reason,
                level,
            )
        }
        "ai.session" => Notification::new(
            format!("AI agent {}", text(data, "session")),
            text(data, "status"),
            if event_failed(data) {
                Level::Error
            } else {
                Level::Info
            },
        ),
        "download.done" => {
            let name = [
                text(data, "path"),
                text(data, "filename"),
                text(data, "url"),
            ]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default();
            Notification::new("Download finished", name, Level::Success)
        }
        "download.failed" => Notification::new(
            "Download failed",
            format!("{} {}", text(data, "url"), text(data, "error")),
            Level::Error,
        ),
        "trigger.fired" => {
            let ok = data.get("ok").and_then(Value::as_bool).unwrap_or(true);
            let script = text(data, "script");
            if ok {
                Notification::new(
                    format!("Trigger {id} ran {script}"),
                    duration(num(data, "duration_ms")),
                    Level::Success,
                )
            } else {
                Notification::new(
                    format!("Trigger {id} failed ({script})"),
                    error_text(data.get("error").unwrap_or(&Value::Null)),
                    Level::Error,
                )
            }
        }
        "net.paused" => Notification::new(
            "Request paused at a breakpoint",
            format!("{} {}", text(data, "method"), text(data, "url")),
            Level::Warning,
        ),
        _ => {
            let body = if data.is_null() {
                String::new()
            } else {
                truncate(&data.to_string(), 300)
            };
            Notification::new(topic.to_string(), body, Level::Info)
        }
    };
    n.topic = Some(topic.to_string());
    n.data = data.clone();
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn topic_patterns() {
        assert!(topic_matches("run.finished", "run.finished"));
        assert!(topic_matches("run.*", "run.finished"));
        assert!(topic_matches("run.*", "run.heal.x"));
        assert!(!topic_matches("run.*", "runs.updated"));
        assert!(!topic_matches("run.*", "run"));
        assert!(topic_matches("*", "anything"));
        assert!(!topic_matches("run.finished", "run.finished2"));
    }

    #[test]
    fn default_rules_route_the_planned_events() {
        let mut config = NotifyConfig::default();
        config.channels.telegram.enabled = true;
        config.channels.telegram.bot_token = "1:x".into();
        config.channels.telegram.chat_id = "5".into();
        let focused = || true;
        let unfocused = || false;

        let (_, ch) = route(&config, "run.finished", &json!({}), focused).unwrap();
        assert_eq!(ch, vec![Channel::Ui, Channel::Os, Channel::Telegram]); // webhook: no url
        assert!(route(&config, "run.heal_needed", &json!({}), focused).is_some());
        let (rule, ch) = route(&config, "run.row_failed", &json!({}), focused).unwrap();
        assert!(rule.aggregate);
        assert_eq!(ch, vec![Channel::Ui]);

        // ai.done only when the window is in the background.
        assert!(route(&config, "ai.done", &json!({}), focused).is_none());
        let (_, ch) = route(&config, "ai.done", &json!({}), unfocused).unwrap();
        assert_eq!(ch, vec![Channel::Os, Channel::Telegram]);

        // trigger.fired only on failure.
        assert!(route(&config, "trigger.fired", &json!({"ok": true}), focused).is_none());
        assert!(route(&config, "trigger.fired", &json!({"ok": false}), focused).is_some());

        assert!(route(&config, "download.done", &json!({}), focused).is_some());
        assert!(route(&config, "tab.updated", &json!({}), focused).is_none());
    }

    #[test]
    fn first_matching_rule_wins_and_disabled_channels_drop_out() {
        let mut config = NotifyConfig::default();
        config.channels.os = false;
        config.rules = vec![
            Rule {
                topic: "net.paused".into(),
                channels: vec![Channel::Os],
                when: When::Always,
                aggregate: false,
            },
            Rule {
                topic: "net.*".into(),
                channels: vec![Channel::Ui, Channel::Ui],
                when: When::Always,
                aggregate: false,
            },
        ];
        // The first rule matches, but its only channel is off → nothing.
        assert!(route(&config, "net.paused", &json!({}), || true).is_none());
        let (_, ch) = route(&config, "net.lagged", &json!({}), || true).unwrap();
        assert_eq!(ch, vec![Channel::Ui]); // de-duplicated
    }

    #[test]
    fn failure_detection() {
        assert!(event_failed(&json!({"ok": false})));
        assert!(event_failed(&json!({"status": "failed"})));
        assert!(event_failed(&json!({"failed": 3})));
        assert!(!event_failed(
            &json!({"ok": true, "failed": 0, "status": "finished"})
        ));
    }

    #[test]
    fn describes_known_events() {
        let n = describe(
            "run.finished",
            &json!({"id": "r1", "status": "finished", "total": 10, "ok": 9, "failed": 1,
                    "cancelled": 0, "needs_heal": 0, "duration_ms": 83_000}),
        );
        assert_eq!(n.title, "Run r1 finished");
        assert_eq!(n.body, "9/10 rows ok, 1 failed · 1m 23s");
        assert_eq!(n.level, Level::Warning);
        assert_eq!(n.topic.as_deref(), Some("run.finished"));

        let n = describe(
            "run.row_failed",
            &json!({"id": "r1", "row": 7, "error": {"code": "NOT_FOUND", "message": "no button"}}),
        );
        assert_eq!(n.title, "Run r1: row 7 failed");
        assert_eq!(n.body, "NOT_FOUND: no button");
        assert_eq!(n.group.unwrap().0, "run.row_failed:r1");

        let n = describe(
            "trigger.fired",
            &json!({"id": "t1", "script": "s", "ok": false, "error": "boom"}),
        );
        assert_eq!(n.title, "Trigger t1 failed (s)");
        assert_eq!(n.level, Level::Error);

        let n = describe("custom.thing", &json!({"a": 1}));
        assert_eq!(n.title, "custom.thing");
        assert_eq!(n.body, r#"{"a":1}"#);
    }
}
