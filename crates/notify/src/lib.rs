//! BrowserMCP notifications: browser UI toasts, Windows toasts, a Telegram bot with answer
//! buttons, and a webhook.
//!
//! * [`config`] — `data/config/notify.json`: channels on/off, rules (event topic → channels),
//!   anti-spam limits;
//! * [`rules`] — which rule an event hits, and the human text for it;
//! * [`limiter`] — dedup, aggregation of bursts (row failures of a 1000-row run), rate limit;
//! * [`heal`] — the `run.heal_needed` question and its buttons → `run.resume` / `run.cancel`;
//! * [`telegram`] — Bot API calls (`sendMessage`, `getUpdates` long polling, …);
//! * [`os`] — Windows toasts and the "is our window in front?" check;
//! * [`commands`] — `notify.config_get/config_set/test/send`, `ui.notify`, `ui.ask_user`,
//!   `ui.answer` (hidden, used by the UI toast buttons).
//!
//! The app starts one [`NotifyService`] (it subscribes to the [`EventBus`] and routes events
//! by the rules), puts it into the registry's `Extensions`, and calls [`attach`] once the
//! registry is shared, so buttons can run commands:
//!
//! ```no_run
//! # async fn demo(data: bmcp_core::DataDir, events: bmcp_core::EventBus,
//! #               backend: std::sync::Arc<dyn bmcp_core::BrowserBackend>) {
//! let mut registry = bmcp_core::Registry::new();
//! bmcp_notify::register(&mut registry);
//! let notify = bmcp_notify::NotifyService::start(
//!     bmcp_notify::NotifyOptions::new(data.config().join("notify.json")),
//!     events,
//! );
//! registry.extensions().insert(notify);
//! let registry = std::sync::Arc::new(registry);
//! bmcp_notify::attach(&registry, backend);
//! # }
//! ```
//!
//! **Events.** `ui.notification {id, title, body, level, actions, sticky, topic}` shows a toast
//! in the UI; `ui.notification_closed {id, choice, label, via}` removes it (a question was
//! answered — maybe on Telegram — or expired). Questions (notifications with buttons) are
//! answered by `ui.answer {id, choice}` from the UI, or by a Telegram button press; the first
//! answer wins.

pub mod commands;
pub mod config;
pub mod heal;
pub mod limiter;
pub mod model;
pub mod os;
pub mod rules;
pub mod telegram;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bmcp_core::{BrowserBackend, Caller, Ctx, Event, EventBus, Registry};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

pub use config::{Channel, NotifyConfig};
pub use model::{Action, Level, Notification};

use crate::limiter::Limiter;
use crate::os::OsToast;
use crate::telegram::{Bot, Press};

/// Event topics this crate emits.
pub mod topics {
    /// `{id, title, body, level, actions: [{id, label}], sticky, topic}` — show a toast.
    pub const NOTIFICATION: &str = "ui.notification";
    /// `{id, choice, label, via}` — the question was answered (or expired): hide it.
    pub const NOTIFICATION_CLOSED: &str = "ui.notification_closed";
}

/// Register the notification commands.
pub fn register(registry: &mut Registry) {
    commands::register(registry);
}

/// Let answer buttons run commands (`run.resume`, …). Call once the registry is shared.
pub fn attach(registry: &Arc<Registry>, backend: Arc<dyn BrowserBackend>) {
    if let Some(service) = registry.extensions().get::<NotifyService>() {
        let _ = service.inner.attached.set(Attached {
            registry: Arc::downgrade(registry),
            backend,
        });
    }
}

/// Shows a Windows toast (swappable for tests).
pub type OsSink = Arc<dyn Fn(&OsToast) -> Result<(), String> + Send + Sync>;
/// "Is the browser window in front?" (swappable for tests).
pub type FocusProbe = Arc<dyn Fn() -> bool + Send + Sync>;

/// How to run the service. [`NotifyOptions::new`] gives the real thing.
#[derive(Clone)]
pub struct NotifyOptions {
    /// `data/config/notify.json`
    pub config_path: PathBuf,
    /// Telegram `getUpdates` long-poll timeout.
    pub poll_timeout_secs: u64,
    /// Wait after a failed Telegram poll.
    pub poll_retry: Duration,
    pub os_sink: OsSink,
    pub focused: FocusProbe,
}

impl NotifyOptions {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        NotifyOptions {
            config_path: config_path.into(),
            poll_timeout_secs: 25,
            poll_retry: Duration::from_secs(5),
            os_sink: Arc::new(os::show),
            focused: Arc::new(os::app_is_foreground),
        }
    }
}

/// What happened to one notification, per channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct Delivery {
    /// Notification id (`n3`).
    pub id: String,
    pub delivered: Vec<Channel>,
    pub failed: Vec<ChannelError>,
    /// Not sent: channel switched off / not configured, or over the rate limit.
    pub skipped: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ChannelError {
    pub channel: Channel,
    pub error: String,
}

/// Answer to a question.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Answer {
    /// The chosen choice text; `null` when nobody answered in time.
    pub choice: Option<String>,
    /// Its position in `choices` (0-based).
    pub index: Option<usize>,
    /// `ui`, `telegram` or `timeout`.
    pub via: String,
}

/// The long-lived notification service. Cheap to clone.
#[derive(Clone)]
pub struct NotifyService {
    inner: Arc<Inner>,
}

struct Inner {
    options: NotifyOptions,
    config: Mutex<NotifyConfig>,
    events: EventBus,
    http: reqwest::Client,
    limiter: Mutex<Limiter>,
    /// Open questions by notification id.
    prompts: Mutex<HashMap<String, Prompt>>,
    next_id: AtomicU64,
    /// Wakes the Telegram poller when a question is waiting.
    poll_wake: tokio::sync::Notify,
    attached: OnceLock<Attached>,
    cancel: CancellationToken,
}

struct Attached {
    registry: Weak<Registry>,
    backend: Arc<dyn BrowserBackend>,
}

struct Prompt {
    notification: Notification,
    channels: Vec<Channel>,
    kind: PromptKind,
    /// The Telegram message showing it, once sent.
    telegram_message: Option<i64>,
}

enum PromptKind {
    /// `ui.ask_user`: the waiting command gets the answer.
    Ask(oneshot::Sender<Answer>),
    /// `run.heal_needed`: the answer runs a run command.
    Heal { run: String, patch: Option<String> },
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `tokio::spawn` that does nothing outside a runtime (e.g. a drop guard during shutdown).
fn spawn(task: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl NotifyService {
    /// Load the config and start the background tasks (event routing, Telegram polling).
    /// Must be called inside a tokio runtime.
    pub fn start(options: NotifyOptions, events: EventBus) -> Self {
        let config = NotifyConfig::load(&options.config_path);
        let http = reqwest::Client::builder()
            .user_agent("BrowserMCP-notify")
            .build()
            .unwrap_or_default();
        // Subscribe before returning, so no event emitted after `start` is missed.
        let rx = events.subscribe();
        let service = NotifyService {
            inner: Arc::new(Inner {
                options,
                config: Mutex::new(config),
                events,
                http,
                limiter: Mutex::default(),
                prompts: Mutex::default(),
                next_id: AtomicU64::new(0),
                poll_wake: tokio::sync::Notify::new(),
                attached: OnceLock::new(),
                cancel: CancellationToken::new(),
            }),
        };
        tokio::spawn(service.clone().event_loop(rx));
        tokio::spawn(service.clone().poll_loop());
        service
    }

    /// Stop the background tasks.
    pub fn shutdown(&self) {
        self.inner.cancel.cancel();
    }

    pub fn config(&self) -> NotifyConfig {
        lock(&self.inner.config).clone()
    }

    /// Replace and save the config.
    pub fn set_config(&self, config: NotifyConfig) -> std::io::Result<()> {
        config.save(&self.inner.options.config_path)?;
        *lock(&self.inner.config) = config;
        self.inner.poll_wake.notify_one();
        Ok(())
    }

    fn next_id(&self) -> String {
        format!(
            "n{}",
            self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1
        )
    }

    /// Ids of the open questions (for tests and diagnostics).
    pub fn open_questions(&self) -> Vec<String> {
        lock(&self.inner.prompts).keys().cloned().collect()
    }

    // ------------------------------------------------------------------ sending

    /// Send `n` to `channels`. Channels that are off are skipped; with `limited`, the rate
    /// limit applies too (questions always pass).
    pub async fn send(&self, mut n: Notification, channels: &[Channel], limited: bool) -> Delivery {
        if n.id.is_empty() {
            n.id = self.next_id();
        }
        let config = self.config();
        let mut delivery = Delivery {
            id: n.id.clone(),
            ..Delivery::default()
        };
        for &channel in channels {
            if !config.enabled(channel) {
                delivery.skipped.push(channel);
                continue;
            }
            let mut this = n.clone();
            if limited {
                let now = now_ms();
                let allowed =
                    lock(&self.inner.limiter).allow(channel, n.is_question(), now, &config.limits);
                match allowed {
                    None => {
                        delivery.skipped.push(channel);
                        continue;
                    }
                    Some(0) => {}
                    Some(dropped) => this
                        .body
                        .push_str(&format!("\n(+{dropped} more suppressed)")),
                }
            }
            match self.deliver(channel, &this, &config).await {
                Ok(()) => delivery.delivered.push(channel),
                Err(error) => {
                    tracing::warn!(channel = channel.as_str(), %error, "notification not sent");
                    delivery.failed.push(ChannelError { channel, error });
                }
            }
        }
        delivery
    }

    /// One channel, no checks (used by `send` and `notify.test`).
    async fn deliver(
        &self,
        channel: Channel,
        n: &Notification,
        config: &NotifyConfig,
    ) -> Result<(), String> {
        match channel {
            Channel::Ui => {
                self.inner.events.emit(topics::NOTIFICATION, n.ui_payload());
                Ok(())
            }
            Channel::Os => {
                let toast = os::toast_for(n);
                let sink = self.inner.options.os_sink.clone();
                tokio::task::spawn_blocking(move || sink(&toast))
                    .await
                    .map_err(|e| format!("toast thread: {e}"))?
            }
            Channel::Telegram => {
                let tg = &config.channels.telegram;
                if !tg.is_configured() {
                    return Err("telegram: bot_token and chat_id are not set".into());
                }
                let message = Bot::new(&self.inner.http, tg).send(n).await?;
                self.remember_telegram_message(&n.id, message);
                Ok(())
            }
            Channel::Webhook => {
                let url = config.channels.webhook.url.trim();
                if url.is_empty() {
                    return Err("webhook: url is not set".into());
                }
                self.inner
                    .http
                    .post(url)
                    .json(&n.webhook_payload(now_ms()))
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map(drop)
                    .map_err(|e| format!("webhook: {}", e.without_url()))
            }
        }
    }

    /// A question went out on Telegram: remember the message, to edit it once answered.
    /// (Answered already in the meantime: its buttons then just say "expired".)
    fn remember_telegram_message(&self, id: &str, message: i64) {
        if let Some(prompt) = lock(&self.inner.prompts).get_mut(id) {
            prompt.telegram_message = Some(message);
        }
    }

    /// Send a test notification to one channel, even when it is switched off.
    pub async fn test(&self, channel: Channel) -> Result<(), String> {
        let mut n = Notification::new(
            "BrowserMCP test",
            format!("Notifications to {} work.", channel.as_str()),
            Level::Info,
        );
        n.id = self.next_id();
        n.topic = Some("notify.test".into());
        self.deliver(channel, &n, &self.config()).await
    }

    // ------------------------------------------------------------------ questions

    /// Ask the user; resolves with the first answer from any channel, or `via: "timeout"`.
    /// Fails when the question could not be shown anywhere.
    pub async fn ask(
        &self,
        question: &str,
        body: &str,
        choices: &[String],
        timeout: Duration,
        channels: &[Channel],
    ) -> Result<Answer, String> {
        let mut n = Notification::new(question, body, Level::Info);
        n.id = self.next_id();
        n.topic = Some("ui.ask_user".into());
        n.actions = choices
            .iter()
            .enumerate()
            .map(|(i, label)| Action::new(format!("c{i}"), label.clone()))
            .collect();
        let (tx, rx) = oneshot::channel();
        self.open_prompt(&n, channels, PromptKind::Ask(tx));
        // If the caller goes away (MCP client cancelled), the question must not stay open.
        let _guard = CloseOnDrop {
            service: self.clone(),
            id: n.id.clone(),
        };

        let delivery = self.send(n.clone(), channels, false).await;
        if delivery.delivered.is_empty() {
            self.close(&n.id, None, "", "error");
            let reasons: Vec<String> = delivery.failed.iter().map(|f| f.error.clone()).collect();
            return Err(if reasons.is_empty() {
                "none of the requested channels is enabled".into()
            } else {
                reasons.join("; ")
            });
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => Ok(answer),
            _ => {
                self.close(&n.id, None, "", "timeout");
                Ok(Answer {
                    choice: None,
                    index: None,
                    via: "timeout".into(),
                })
            }
        }
    }

    fn open_prompt(&self, n: &Notification, channels: &[Channel], kind: PromptKind) {
        lock(&self.inner.prompts).insert(
            n.id.clone(),
            Prompt {
                notification: n.clone(),
                channels: channels.to_vec(),
                kind,
                telegram_message: None,
            },
        );
        if channels.contains(&Channel::Telegram) {
            self.inner.poll_wake.notify_one();
        }
    }

    /// Answer question `id` with action `choice` (`via` = `ui`, `telegram`, …). Returns the
    /// button's label. Fails when the question is gone (answered/expired) or the choice is
    /// unknown.
    pub fn answer(&self, id: &str, choice: &str, via: &str) -> Result<String, String> {
        let mut prompts = lock(&self.inner.prompts);
        let prompt = prompts
            .get(id)
            .ok_or("this question was already answered or has expired")?;
        let label = prompt
            .notification
            .actions
            .iter()
            .find(|a| a.id == choice)
            .map(|a| a.label.clone())
            .ok_or_else(|| format!("'{choice}' is not one of the choices"))?;

        // Heal buttons that do not answer: "Open in browser", "Show patch".
        if let PromptKind::Heal { run, patch } = &prompt.kind
            && !heal::is_final(choice)
        {
            let message = prompt.telegram_message;
            let text = prompt.notification.telegram_text();
            let actions = telegram_actions(&prompt.notification);
            let followup = match choice {
                heal::SHOW => patch.as_deref().map(|code| heal::patch_message(run, code)),
                _ => None,
            };
            drop(prompts);
            let service = self.clone();
            let config = self.config().channels.telegram;
            spawn(async move {
                let bot = Bot::new(&service.inner.http, &config);
                let result = match (followup, message) {
                    (Some(code), _) => bot.send_text(&code, message).await.map(drop),
                    (None, Some(message)) => {
                        let text = format!("{text}\n\nWaiting for you in BrowserMCP.");
                        bot.edit(message, &text, Some(&actions)).await
                    }
                    (None, None) => Ok(()),
                };
                if let Err(err) = result {
                    tracing::warn!(%err, "telegram follow-up failed");
                }
            });
            return Ok(label);
        }

        let prompt = prompts.remove(id).expect("checked above");
        drop(prompts);
        self.finish(prompt, Some(choice), &label, via);
        Ok(label)
    }

    /// Close an open question without an answer (timeout, resolved elsewhere).
    fn close(&self, id: &str, choice: Option<&str>, label: &str, via: &str) {
        let prompt = lock(&self.inner.prompts).remove(id);
        if let Some(prompt) = prompt {
            self.finish(prompt, choice, label, via);
        }
    }

    /// After a question left the open set: tell the UI, update Telegram, deliver the answer.
    fn finish(&self, prompt: Prompt, choice: Option<&str>, label: &str, via: &str) {
        let id = prompt.notification.id.clone();
        self.inner.events.emit(
            topics::NOTIFICATION_CLOSED,
            json!({ "id": id, "choice": choice, "label": label, "via": via }),
        );
        if let Some(message) = prompt.telegram_message {
            let outcome = match choice {
                Some(_) => format!("Answered: {label} (via {via})"),
                None if via == "timeout" => "Expired without an answer.".to_string(),
                None => format!("Closed ({via})."),
            };
            let text = format!("{}\n\n{outcome}", prompt.notification.telegram_text());
            let service = self.clone();
            spawn(async move {
                let config = service.config().channels.telegram;
                if let Err(err) = Bot::new(&service.inner.http, &config)
                    .edit(message, &text, None)
                    .await
                {
                    tracing::warn!(%err, "cannot update the telegram message");
                }
            });
        }
        match prompt.kind {
            PromptKind::Ask(tx) => {
                if let Some(choice) = choice {
                    let index = choice.strip_prefix('c').and_then(|i| i.parse().ok());
                    let _ = tx.send(Answer {
                        choice: Some(label.to_string()),
                        index,
                        via: via.to_string(),
                    });
                }
            }
            PromptKind::Heal { run, patch } => {
                if let Some((name, args)) =
                    choice.and_then(|c| heal::command_for(c, &run, patch.as_deref()))
                {
                    let service = self.clone();
                    let label = label.to_string();
                    spawn(async move { service.run_command(&name, args, &label).await });
                }
            }
        }
    }

    /// Run a registry command for a button (as the user: `Caller::Ui`). Failures show up as
    /// an error toast.
    async fn run_command(&self, name: &str, args: Value, label: &str) {
        let result = match self.inner.attached.get() {
            None => Err("the command registry is not attached".to_string()),
            Some(attached) => match attached.registry.upgrade() {
                None => Err("the browser is shutting down".to_string()),
                Some(registry) => {
                    let ctx = Ctx::new(
                        Caller::Ui,
                        attached.backend.clone(),
                        self.inner.events.clone(),
                    );
                    registry
                        .call(ctx, name, args)
                        .await
                        .map(drop)
                        .map_err(|e| e.message)
                }
            },
        };
        if let Err(err) = result {
            tracing::warn!(command = name, %err, "notification button failed");
            let n = Notification::new(format!("\"{label}\" failed"), err, Level::Error);
            self.send(n, &[Channel::Ui], false).await;
        }
    }

    // ------------------------------------------------------------------ event routing

    async fn event_loop(self, mut rx: broadcast::Receiver<Event>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                event = rx.recv() => match event {
                    Ok(event) => self.on_event(event),
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "notify fell behind the event bus");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => self.flush_summaries(),
                _ = self.inner.cancel.cancelled() => break,
            }
        }
    }

    fn on_event(&self, event: Event) {
        let topic = event.topic.as_str();
        // Our own events must never trigger notifications (loops).
        if topic.starts_with("ui.") {
            return;
        }
        self.close_resolved_heals(topic, &event.data);
        let config = self.config();
        let focused = self.inner.options.focused.clone();
        let Some((rule, channels)) = rules::route(&config, topic, &event.data, || focused()) else {
            return;
        };
        let mut n = rules::describe(topic, &event.data);
        n.id = self.next_id();
        {
            let now = now_ms();
            let mut limiter = lock(&self.inner.limiter);
            if !limiter.dedup(&n, now, &config.limits) {
                return;
            }
            if rule.aggregate && !limiter.aggregate(&n, &channels, now) {
                return;
            }
        }
        if topic == "run.heal_needed" {
            let run = event.data.get("id").and_then(Value::as_str).unwrap_or("");
            let kind = PromptKind::Heal {
                run: run.to_string(),
                patch: heal::patch(&event.data).map(String::from),
            };
            self.open_prompt(&n, &channels, kind);
        }
        let service = self.clone();
        spawn(async move {
            service.send(n, &channels, true).await;
        });
    }

    /// A heal question is moot once the run went on by other means: someone called
    /// `run.resume` / `run.cancel` / `run.heal_apply` for it, or it finished.
    fn close_resolved_heals(&self, topic: &str, data: &Value) {
        let run = match topic {
            "run.finished" => data.get("id"),
            "command.started"
                if matches!(
                    data.get("name").and_then(Value::as_str),
                    Some("run.resume" | "run.cancel" | "run.heal_apply")
                ) =>
            {
                data.get("args").and_then(|a| a.get("id"))
            }
            _ => None,
        };
        let Some(run) = run.and_then(Value::as_str) else {
            return;
        };
        let ids: Vec<String> = lock(&self.inner.prompts)
            .iter()
            .filter(|(_, p)| matches!(&p.kind, PromptKind::Heal { run: r, .. } if r == run))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.close(&id, None, "", "elsewhere");
        }
    }

    fn flush_summaries(&self) {
        let limits = self.config().limits;
        let summaries = lock(&self.inner.limiter).flush(now_ms(), &limits);
        for (mut n, channels) in summaries {
            n.id = self.next_id();
            let service = self.clone();
            spawn(async move {
                service.send(n, &channels, true).await;
            });
        }
    }

    // ------------------------------------------------------------------ Telegram polling

    /// Long-poll `getUpdates` while a question waits for a Telegram answer; idle otherwise.
    async fn poll_loop(self) {
        let mut offset = 0i64;
        let cancel = self.inner.cancel.clone();
        loop {
            let config = self.config().channels.telegram;
            if !(config.usable() && self.waiting_on_telegram()) {
                tokio::select! {
                    _ = self.inner.poll_wake.notified() => continue,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                    _ = cancel.cancelled() => break,
                }
            }
            let bot = Bot::new(&self.inner.http, &config);
            let polled = tokio::select! {
                polled = bot.presses(offset, self.inner.options.poll_timeout_secs) => polled,
                _ = cancel.cancelled() => break,
            };
            match polled {
                Ok((presses, next)) => {
                    offset = next;
                    for press in presses {
                        self.on_press(&bot, press).await;
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "telegram polling failed");
                    tokio::select! {
                        _ = tokio::time::sleep(self.inner.options.poll_retry) => {}
                        _ = cancel.cancelled() => break,
                    }
                }
            }
        }
    }

    fn waiting_on_telegram(&self) -> bool {
        lock(&self.inner.prompts)
            .values()
            .any(|p| p.channels.contains(&Channel::Telegram))
    }

    async fn on_press(&self, bot: &Bot<'_>, press: Press) {
        let config = self.config().channels.telegram;
        let reply = if !telegram::from_our_chat(&press, &config) {
            tracing::warn!(
                chat = press.chat_id,
                "telegram button press from another chat ignored"
            );
            "Not allowed.".to_string()
        } else {
            match telegram::parse_callback(&press.data) {
                Some((id, choice)) if self.press_matches_message(id, press.message_id) => {
                    let via = "telegram";
                    match self.answer(id, choice, via) {
                        Ok(label) => label,
                        Err(err) => err,
                    }
                }
                _ => "This question has expired.".to_string(),
            }
        };
        if let Err(err) = bot.answer_press(&press.callback_id, &reply).await {
            tracing::warn!(%err, "answerCallbackQuery failed");
        }
    }

    /// A press must come from the message that shows this question (ids restart with every
    /// browser start, so an old message's `n3` is not today's `n3`).
    fn press_matches_message(&self, id: &str, message: Option<i64>) -> bool {
        match lock(&self.inner.prompts).get(id) {
            Some(prompt) => match prompt.telegram_message {
                Some(ours) => message == Some(ours),
                None => true, // pressed before we stored the id: only possible for a new message
            },
            None => true, // `answer` reports "expired"
        }
    }
}

/// Closes a question when `ask` ends in any way (a no-op when it was answered).
struct CloseOnDrop {
    service: NotifyService,
    id: String,
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.service.close(&self.id, None, "", "cancelled");
    }
}

/// The question's buttons as Telegram shows them (`<id>|<action>` callback data).
fn telegram_actions(n: &Notification) -> Vec<Action> {
    n.actions
        .iter()
        .map(|a| Action {
            id: telegram::callback_data(&n.id, &a.id),
            ..a.clone()
        })
        .collect()
}
