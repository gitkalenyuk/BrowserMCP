//! Registry commands (toolset `ui`; `ui.answer` is hidden: only the UI's toast buttons use it).

use std::sync::Arc;
use std::time::Duration;

use bmcp_core::command::{CmdError, Command, Ctx, ErrorCode, Meta, Registry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{self, Channel, Limits, NotifyConfig, Rule};
use crate::{Answer, Delivery, Level, Notification, NotifyService};

pub fn register(registry: &mut Registry) {
    registry
        .add::<ConfigGet>()
        .add::<ConfigSet>()
        .add::<NotifyTest>()
        .add::<NotifySend>()
        .add::<UiNotify>()
        .add::<UiAskUser>()
        .add::<UiAnswer>();
}

fn service(ctx: &Ctx) -> Result<Arc<NotifyService>, CmdError> {
    ctx.extensions.get::<NotifyService>().ok_or_else(|| {
        CmdError::new(
            ErrorCode::NotImplemented,
            "notifications are not running in this browser",
        )
    })
}

/// Requested channels that are switched on, or `default` when none were requested.
fn pick_channels(
    config: &NotifyConfig,
    requested: Option<Vec<Channel>>,
    default: &[Channel],
) -> Vec<Channel> {
    let mut out = Vec::new();
    for channel in requested.unwrap_or_else(|| default.to_vec()) {
        if !out.contains(&channel) {
            out.push(channel);
        }
    }
    out.retain(|&c| config.enabled(c));
    out
}

// ------------------------------------------------------------------ notify.config_*

pub struct ConfigGet;

impl Command for ConfigGet {
    const NAME: &'static str = "notify.config_get";
    type Args = bmcp_core::command::Empty;
    type Output = NotifyConfig;

    fn meta() -> Meta {
        Meta::new(
            "Notification settings (data/config/notify.json): channels ui/os/telegram/webhook, \
             rules (event topic → channels, when: always|unfocused|failed, aggregate) and \
             anti-spam limits. The Telegram bot token is masked.",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, _args: Self::Args) -> Result<NotifyConfig, CmdError> {
        Ok(service(&ctx)?.config().masked())
    }
}

pub struct ConfigSet;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TelegramPatch {
    pub enabled: Option<bool>,
    /// Bot token from @BotFather. The masked value from config_get ("…abcd") keeps the stored one.
    pub bot_token: Option<String>,
    /// Your chat id with the bot (number) or "@channel".
    pub chat_id: Option<String>,
    pub api_base: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookPatch {
    pub enabled: Option<bool>,
    pub url: Option<String>,
}

/// Only the given fields change.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigSetArgs {
    /// Toasts in the browser UI.
    pub ui: Option<bool>,
    /// Windows toast notifications.
    pub os: Option<bool>,
    pub telegram: Option<TelegramPatch>,
    pub webhook: Option<WebhookPatch>,
    /// Replaces all rules. Topic: exact, "prefix.*" or "*"; first match wins.
    pub rules: Option<Vec<Rule>>,
    pub limits: Option<Limits>,
}

/// Apply a patch to a config.
pub fn apply_patch(config: &mut NotifyConfig, args: ConfigSetArgs) -> Result<(), CmdError> {
    let ch = &mut config.channels;
    if let Some(ui) = args.ui {
        ch.ui = ui;
    }
    if let Some(os) = args.os {
        ch.os = os;
    }
    if let Some(tg) = args.telegram {
        if let Some(enabled) = tg.enabled {
            ch.telegram.enabled = enabled;
        }
        if let Some(token) = tg.bot_token
            && !config::is_masked(&token)
        {
            ch.telegram.bot_token = token.trim().to_string();
        }
        if let Some(chat) = tg.chat_id {
            ch.telegram.chat_id = chat.trim().to_string();
        }
        if let Some(base) = tg.api_base {
            if !(base.starts_with("https://") || base.starts_with("http://")) {
                return Err(CmdError::invalid_params(
                    "telegram.api_base must be an http(s) URL",
                ));
            }
            ch.telegram.api_base = base;
        }
    }
    if let Some(hook) = args.webhook {
        if let Some(enabled) = hook.enabled {
            ch.webhook.enabled = enabled;
        }
        if let Some(url) = hook.url {
            let url = url.trim().to_string();
            if !url.is_empty() && !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(CmdError::invalid_params(
                    "webhook.url must be an http(s) URL",
                ));
            }
            ch.webhook.url = url;
        }
    }
    if let Some(rules) = args.rules {
        if let Some(bad) = rules.iter().find(|r| r.topic.trim().is_empty()) {
            return Err(CmdError::invalid_params(format!(
                "rule with an empty topic: {bad:?}"
            )));
        }
        config.rules = rules;
    }
    if let Some(limits) = args.limits {
        config.limits = limits;
    }
    Ok(())
}

impl Command for ConfigSet {
    const NAME: &'static str = "notify.config_set";
    type Args = ConfigSetArgs;
    type Output = NotifyConfig;

    fn meta() -> Meta {
        Meta::new(
            "Change notification settings (only the fields given). Returns the new settings \
             (token masked). Test a channel afterwards with notify_test.",
            Toolset::Ui,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: ConfigSetArgs) -> Result<NotifyConfig, CmdError> {
        let service = service(&ctx)?;
        let mut config = service.config();
        apply_patch(&mut config, args)?;
        service
            .set_config(config.clone())
            .map_err(|e| CmdError::internal(format!("cannot save notify.json: {e}")))?;
        Ok(config.masked())
    }
}

// ------------------------------------------------------------------ notify.test / send

pub struct NotifyTest;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestArgs {
    pub channel: Channel,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TestOutput {
    pub channel: Channel,
    pub ok: bool,
    pub error: Option<String>,
}

impl Command for NotifyTest {
    const NAME: &'static str = "notify.test";
    type Args = TestArgs;
    type Output = TestOutput;

    fn meta() -> Meta {
        Meta::new(
            "Send a test notification to one channel (even if it is switched off) and report \
             whether it worked.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: TestArgs) -> Result<TestOutput, CmdError> {
        let result = service(&ctx)?.test(args.channel).await;
        Ok(TestOutput {
            channel: args.channel,
            ok: result.is_ok(),
            error: result.err(),
        })
    }
}

pub struct NotifySend;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendArgs {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub level: Level,
    /// Default: every enabled channel.
    pub channels: Option<Vec<Channel>>,
}

impl Command for NotifySend {
    const NAME: &'static str = "notify.send";
    type Args = SendArgs;
    type Output = Delivery;

    fn meta() -> Meta {
        Meta::new(
            "Send a notification (default: to every enabled channel: ui, os, telegram, \
             webhook). Returns which channels got it.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SendArgs) -> Result<Delivery, CmdError> {
        let service = service(&ctx)?;
        let channels = pick_channels(&service.config(), args.channels, &Channel::ALL);
        let n = Notification::new(args.title, args.body, args.level);
        Ok(service.send(n, &channels, true).await)
    }
}

// ------------------------------------------------------------------ ui.notify / ask_user

pub struct UiNotify;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiNotifyArgs {
    /// Message for the user.
    pub text: String,
    /// Short heading; default "BrowserMCP".
    pub title: Option<String>,
    #[serde(default)]
    pub level: Level,
    /// Default ["ui"]; add "os" / "telegram" to reach the user away from the browser.
    pub channels: Option<Vec<Channel>>,
}

impl Command for UiNotify {
    const NAME: &'static str = "ui.notify";
    type Args = UiNotifyArgs;
    type Output = Delivery;

    fn meta() -> Meta {
        Meta::new(
            "Show the user a notification (a toast in the browser by default; channels can \
             add os / telegram / webhook). Does not wait for a reply: use ui_ask_user for \
             questions.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: UiNotifyArgs) -> Result<Delivery, CmdError> {
        let service = service(&ctx)?;
        let channels = pick_channels(&service.config(), args.channels, &[Channel::Ui]);
        let title = args.title.unwrap_or_else(|| "BrowserMCP".into());
        let n = Notification::new(title, args.text, args.level);
        Ok(service.send(n, &channels, true).await)
    }
}

pub struct UiAskUser;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskArgs {
    pub question: String,
    /// Optional details under the question.
    #[serde(default)]
    pub details: String,
    /// 1–8 answer buttons, e.g. ["Yes", "No"].
    pub choices: Vec<String>,
    /// How long to wait (default 300, max 3600). No answer → choice null, via "timeout".
    pub timeout_secs: Option<u64>,
    /// Default: ui, plus telegram and os when they are enabled (os only shows the question).
    pub channels: Option<Vec<Channel>>,
}

pub const MAX_CHOICES: usize = 8;

impl Command for UiAskUser {
    const NAME: &'static str = "ui.ask_user";
    type Args = AskArgs;
    type Output = Answer;

    fn meta() -> Meta {
        Meta::new(
            "Ask the user a question with answer buttons (browser toast, and Telegram when \
             enabled) and wait for the first answer. Returns {choice, index, via} (via: ui | \
             telegram | timeout; choice null on timeout).",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: AskArgs) -> Result<Answer, CmdError> {
        let question = args.question.trim();
        if question.is_empty() {
            return Err(CmdError::invalid_params("question is empty"));
        }
        let choices: Vec<String> = args.choices.iter().map(|c| c.trim().to_string()).collect();
        if choices.is_empty() || choices.len() > MAX_CHOICES {
            return Err(CmdError::invalid_params(format!(
                "give 1 to {MAX_CHOICES} choices"
            )));
        }
        if choices.iter().any(String::is_empty) {
            return Err(CmdError::invalid_params("choices must not be empty"));
        }
        for (i, c) in choices.iter().enumerate() {
            if choices[..i].contains(c) {
                return Err(CmdError::invalid_params(format!(
                    "choice '{c}' is listed twice"
                )));
            }
        }
        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(300).clamp(1, 3600));
        let service = service(&ctx)?;
        let channels = pick_channels(
            &service.config(),
            args.channels,
            &[Channel::Ui, Channel::Telegram, Channel::Os],
        );
        service
            .ask(question, &args.details, &choices, timeout, &channels)
            .await
            .map_err(|e| CmdError::invalid_params(format!("cannot ask the user: {e}")))
    }
}

// ------------------------------------------------------------------ ui.answer (hidden)

pub struct UiAnswer;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerArgs {
    /// Notification id from ui.notification.
    pub id: String,
    /// The pressed action's id.
    pub choice: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AnswerOutput {
    pub id: String,
    /// The pressed button's text.
    pub label: String,
}

impl Command for UiAnswer {
    const NAME: &'static str = "ui.answer";
    type Args = AnswerArgs;
    type Output = AnswerOutput;

    fn meta() -> Meta {
        Meta::new(
            "UI plumbing: a notification button was pressed.",
            Toolset::Hidden,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: AnswerArgs) -> Result<AnswerOutput, CmdError> {
        let label = service(&ctx)?
            .answer(&args.id, &args.choice, "ui")
            .map_err(|e| CmdError::new(ErrorCode::NotFound, e))?;
        Ok(AnswerOutput { id: args.id, label })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patch_changes_only_given_fields_and_keeps_masked_token() {
        let mut config = NotifyConfig::default();
        config.channels.telegram.bot_token = "123:secret".into();
        let args: ConfigSetArgs = serde_json::from_value(json!({
            "os": false,
            "telegram": {"enabled": true, "bot_token": "…cret", "chat_id": " 42 "},
        }))
        .unwrap();
        apply_patch(&mut config, args).unwrap();
        assert!(config.channels.ui);
        assert!(!config.channels.os);
        assert!(config.channels.telegram.enabled);
        assert_eq!(config.channels.telegram.bot_token, "123:secret");
        assert_eq!(config.channels.telegram.chat_id, "42");
        assert_eq!(config.rules, NotifyConfig::default().rules);

        let bad: ConfigSetArgs =
            serde_json::from_value(json!({"webhook": {"url": "ftp://x"}})).unwrap();
        assert!(apply_patch(&mut config, bad).is_err());
        assert!(serde_json::from_value::<ConfigSetArgs>(json!({"typo": 1})).is_err());
    }

    #[test]
    fn channel_picking() {
        let config = NotifyConfig::default(); // ui + os on
        assert_eq!(
            pick_channels(&config, None, &Channel::ALL),
            vec![Channel::Ui, Channel::Os]
        );
        assert_eq!(
            pick_channels(
                &config,
                Some(vec![Channel::Telegram, Channel::Ui, Channel::Ui]),
                &[]
            ),
            vec![Channel::Ui]
        );
    }
}
