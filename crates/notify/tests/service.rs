//! End-to-end tests of the notification service against a mock Telegram Bot API (wiremock):
//! no real bot, no real messages.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::command::{CmdError, Command, Ctx, Meta, Toolset};
use bmcp_core::testing::FakeBackend;
use bmcp_core::{Caller, Event, EventBus, Registry};
use bmcp_notify::os::OsToast;
use bmcp_notify::{NotifyConfig, NotifyOptions, NotifyService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TOKEN: &str = "123456:TEST-TOKEN";
const CHAT: i64 = 777;

/// Upper bound for every wait: only reached when something is really broken, never because
/// the machine is busy (a full workspace test run loads it heavily).
const PATIENCE: Duration = Duration::from_secs(30);

/// Poll `check` until it returns `Some`; panic with `what` after [`PATIENCE`].
async fn until<T>(what: &str, mut check: impl AsyncFnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ------------------------------------------------------------------ mock Bot API

/// Records every Bot API call and answers `getUpdates` from a queue.
#[derive(Clone, Default)]
struct FakeTelegram {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    updates: Arc<Mutex<VecDeque<Value>>>,
}

impl Respond for FakeTelegram {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let method = req.url.path().rsplit('/').next().unwrap_or("").to_string();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let count = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((method.clone(), body));
            calls.iter().filter(|(m, _)| m == "sendMessage").count() as i64
        };
        match method.as_str() {
            "sendMessage" => ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "result": {"message_id": 100 + count}})),
            "getUpdates" => {
                let pending: Vec<Value> = self.updates.lock().unwrap().drain(..).collect();
                let reply = ResponseTemplate::new(200)
                    .set_body_json(json!({"ok": true, "result": pending}));
                // The real API blocks while there is nothing; do not spin.
                if pending.is_empty() {
                    reply.set_delay(Duration::from_millis(30))
                } else {
                    reply
                }
            }
            _ => ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": true})),
        }
    }
}

impl FakeTelegram {
    fn calls(&self, method: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, body)| body.clone())
            .collect()
    }

    async fn wait_call(&self, method: &str, n: usize) -> Vec<Value> {
        let what = format!("{n}x {method}");
        until(&what, async || {
            let calls = self.calls(method);
            (calls.len() >= n).then_some(calls)
        })
        .await
    }

    /// Queue a button press on `message_id` with `data`, from `chat`.
    fn press(&self, update_id: i64, chat: i64, message_id: i64, data: &str) {
        self.updates.lock().unwrap().push_back(json!({
            "update_id": update_id,
            "callback_query": {
                "id": format!("cb{update_id}"),
                "data": data,
                "from": {"id": chat, "first_name": "Ann"},
                "message": {"message_id": message_id, "chat": {"id": chat, "type": "private"}},
            }
        }));
    }
}

// ------------------------------------------------------------------ fake run commands

/// Records `run.resume` / `run.cancel` calls made by the heal buttons.
#[derive(Default)]
struct RunCalls(Mutex<Vec<String>>);

#[derive(Deserialize, JsonSchema)]
struct RunIdArgs {
    id: String,
}

#[derive(Serialize, JsonSchema)]
struct RunOut {
    id: String,
}

macro_rules! fake_run_command {
    ($ty:ident, $name:literal) => {
        struct $ty;
        impl Command for $ty {
            const NAME: &'static str = $name;
            type Args = RunIdArgs;
            type Output = RunOut;
            fn meta() -> Meta {
                Meta::new("test double", Toolset::Run)
            }
            async fn run(ctx: Ctx, args: RunIdArgs) -> Result<RunOut, CmdError> {
                let calls = ctx.extensions.get::<RunCalls>().unwrap();
                calls
                    .0
                    .lock()
                    .unwrap()
                    .push(format!("{} {}", $name, args.id));
                Ok(RunOut { id: args.id })
            }
        }
    };
}
fake_run_command!(FakeResume, "run.resume");
fake_run_command!(FakeCancel, "run.cancel");

#[derive(Deserialize, JsonSchema)]
struct ProposalArgs {
    proposal_id: String,
    #[allow(dead_code)]
    feedback: Option<String>,
}

macro_rules! fake_heal_command {
    ($ty:ident, $name:literal) => {
        struct $ty;
        impl Command for $ty {
            const NAME: &'static str = $name;
            type Args = ProposalArgs;
            type Output = RunOut;
            fn meta() -> Meta {
                Meta::new("test double", Toolset::Run)
            }
            async fn run(ctx: Ctx, args: ProposalArgs) -> Result<RunOut, CmdError> {
                let calls = ctx.extensions.get::<RunCalls>().unwrap();
                calls
                    .0
                    .lock()
                    .unwrap()
                    .push(format!("{} {}", $name, args.proposal_id));
                Ok(RunOut {
                    id: args.proposal_id,
                })
            }
        }
    };
}
fake_heal_command!(FakeAccept, "run.heal_accept");
fake_heal_command!(FakeReject, "run.heal_reject");

// ------------------------------------------------------------------ environment

struct Env {
    service: NotifyService,
    events: EventBus,
    rx: broadcast::Receiver<Event>,
    registry: Arc<Registry>,
    telegram: FakeTelegram,
    server: MockServer,
    toasts: Arc<Mutex<Vec<OsToast>>>,
    _dir: tempfile::TempDir,
}

/// Service with telegram on (pointing at the mock), OS toasts recorded, window "focused".
async fn env(edit: impl FnOnce(&mut NotifyConfig, &str)) -> Env {
    let server = MockServer::start().await;
    let telegram = FakeTelegram::default();
    Mock::given(method("POST"))
        .and(path_regex(format!(r"^/bot{}/\w+$", regex_escape(TOKEN))))
        .respond_with(telegram.clone())
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("notify.json");
    let mut config = NotifyConfig::default();
    config.channels.telegram.enabled = true;
    config.channels.telegram.bot_token = TOKEN.into();
    config.channels.telegram.chat_id = CHAT.to_string();
    config.channels.telegram.api_base = server.uri();
    edit(&mut config, &server.uri());
    config.save(&config_path).unwrap();

    let toasts: Arc<Mutex<Vec<OsToast>>> = Arc::default();
    let mut options = NotifyOptions::new(&config_path);
    options.poll_timeout_secs = 1;
    options.poll_retry = Duration::from_millis(50);
    let sink = toasts.clone();
    options.os_sink = Arc::new(move |t: &OsToast| {
        sink.lock().unwrap().push(t.clone());
        Ok(())
    });
    options.focused = Arc::new(|| true);

    let events = EventBus::new();
    let rx = events.subscribe();
    let service = NotifyService::start(options, events.clone());

    let mut registry = Registry::new();
    bmcp_notify::register(&mut registry);
    registry
        .add::<FakeResume>()
        .add::<FakeCancel>()
        .add::<FakeAccept>()
        .add::<FakeReject>();
    registry.extensions().insert(service.clone());
    registry.extensions().insert(RunCalls::default());
    let registry = Arc::new(registry);
    bmcp_notify::attach(&registry, FakeBackend::new());

    Env {
        service,
        events,
        rx,
        registry,
        telegram,
        server,
        toasts,
        _dir: dir,
    }
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_string()
            } else {
                format!("\\{c}")
            }
        })
        .collect()
}

impl Env {
    fn call(&self, name: &str, args: Value) -> tokio::task::JoinHandle<Result<Value, CmdError>> {
        let ctx = Ctx::new(Caller::Rest, FakeBackend::new(), self.events.clone());
        tokio::spawn(self.registry.call(ctx, name, args))
    }

    /// Next event with `topic` (skipping others).
    async fn event(&mut self, topic: &str) -> Value {
        let wait = async {
            loop {
                let event = self.rx.recv().await.unwrap();
                if event.topic == topic {
                    return event.data;
                }
            }
        };
        tokio::time::timeout(PATIENCE, wait)
            .await
            .unwrap_or_else(|_| panic!("no {topic} event"))
    }

    fn run_calls(&self) -> Vec<String> {
        self.registry
            .extensions()
            .get::<RunCalls>()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .clone()
    }
}

fn button_data(send: &Value, label: &str) -> String {
    send["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row.as_array().unwrap().iter())
        .find(|b| b["text"] == label)
        .unwrap_or_else(|| panic!("no button {label} in {send}"))["callback_data"]
        .as_str()
        .unwrap()
        .to_string()
}

fn button_labels(send: &Value) -> Vec<String> {
    send["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row.as_array().unwrap().iter())
        .map(|b| b["text"].as_str().unwrap().to_string())
        .collect()
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn ask_user_is_answered_from_telegram() {
    let mut env = env(|_, _| {}).await;
    let ask = env.call(
        "ui.ask_user",
        json!({"question": "Deploy now?", "choices": ["Yes", "No"], "timeout_secs": 20}),
    );

    // The UI toast and the Telegram message both show the question.
    let toast = env.event("ui.notification").await;
    assert_eq!(toast["title"], "Deploy now?");
    assert_eq!(toast["sticky"], true);
    assert_eq!(
        toast["actions"],
        json!([{"id": "c0", "label": "Yes"}, {"id": "c1", "label": "No"}])
    );
    let send = env.telegram.wait_call("sendMessage", 1).await.remove(0);
    assert_eq!(send["chat_id"], json!(CHAT));
    assert_eq!(send["text"], "Deploy now?");
    assert_eq!(button_labels(&send), ["Yes", "No"]);
    let id = toast["id"].as_str().unwrap();
    assert_eq!(button_data(&send, "No"), format!("{id}|c1"));

    // A press from another chat does not count; ours does.
    env.telegram.press(1, 999, 101, &button_data(&send, "Yes"));
    env.telegram.press(2, CHAT, 101, &button_data(&send, "No"));
    let answer = tokio::time::timeout(PATIENCE, ask)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        answer,
        json!({"choice": "No", "index": 1, "via": "telegram"})
    );

    // The UI toast closes, the presses get answered and the message loses its buttons.
    let closed = env.event("ui.notification_closed").await;
    assert_eq!(
        closed,
        json!({"id": id, "choice": "c1", "label": "No", "via": "telegram"})
    );
    let answers = env.telegram.wait_call("answerCallbackQuery", 2).await;
    assert_eq!(
        answers[0],
        json!({"callback_query_id": "cb1", "text": "Not allowed."})
    );
    assert_eq!(
        answers[1],
        json!({"callback_query_id": "cb2", "text": "No"})
    );
    let edit = env.telegram.wait_call("editMessageText", 1).await.remove(0);
    assert_eq!(edit["message_id"], 101);
    assert_eq!(edit["text"], "Deploy now?\n\nAnswered: No (via telegram)");
    assert!(edit.get("reply_markup").is_none());
    assert!(env.service.open_questions().is_empty());
}

#[tokio::test]
async fn ask_user_is_answered_from_the_ui_first() {
    let mut env = env(|_, _| {}).await;
    let ask = env.call(
        "ui.ask_user",
        json!({"question": "Continue?", "choices": ["Go", "Wait"]}),
    );
    let toast = env.event("ui.notification").await;
    let id = toast["id"].as_str().unwrap().to_string();
    env.telegram.wait_call("sendMessage", 1).await;

    let answered = env
        .call("ui.answer", json!({"id": id, "choice": "c0"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answered, json!({"id": id, "label": "Go"}));
    let answer = ask.await.unwrap().unwrap();
    assert_eq!(answer, json!({"choice": "Go", "index": 0, "via": "ui"}));

    // A second answer fails: the first one won.
    let again = env
        .call("ui.answer", json!({"id": id, "choice": "c1"}))
        .await
        .unwrap();
    assert!(again.is_err());
    let edit = env.telegram.wait_call("editMessageText", 1).await.remove(0);
    assert_eq!(edit["text"], "Continue?\n\nAnswered: Go (via ui)");
}

#[tokio::test]
async fn ask_user_times_out() {
    let mut env = env(|c, _| c.channels.telegram.enabled = false).await;
    let answer = env
        .call(
            "ui.ask_user",
            json!({"question": "Anyone?", "choices": ["Yes"], "timeout_secs": 1}),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        answer,
        json!({"choice": null, "index": null, "via": "timeout"})
    );
    let closed = env.event("ui.notification_closed").await;
    assert_eq!(closed["via"], "timeout");
    assert!(env.telegram.calls("sendMessage").is_empty());
    // OS channel only shows the question.
    assert_eq!(
        env.toasts.lock().unwrap()[0].body,
        "Answer in BrowserMCP: Yes"
    );

    let bad = env
        .call("ui.ask_user", json!({"question": "?", "choices": []}))
        .await
        .unwrap();
    assert!(bad.is_err());
}

#[tokio::test]
async fn heal_needed_buttons_run_the_run_commands() {
    let mut env = env(|_, _| {}).await;
    env.events.emit(
        "run.heal_needed",
        json!({"id": "r1", "row": 7, "script": "login", "step": "page.act", "line": 14,
               "error": {"code": "NOT_FOUND", "message": "no Sign in button"}}),
    );

    // UI: Skip / Stop (no "Open in browser": you are in the browser). Telegram: all three.
    let toast = env.event("ui.notification").await;
    assert_eq!(toast["title"], "Run r1 needs a fix");
    assert_eq!(
        toast["actions"],
        json!([{"id": "skip", "label": "Skip row"}, {"id": "stop", "label": "Stop run"}])
    );
    let send = env.telegram.wait_call("sendMessage", 1).await.remove(0);
    assert_eq!(
        send["text"],
        "[warning] Run r1 needs a fix\nRow 7 failed at step page.act (line 14): NOT_FOUND: no Sign in button\nScript: login"
    );
    assert_eq!(
        button_labels(&send),
        ["Skip row", "Stop run", "Open in browser"]
    );
    // OS toast too (default rule), without the remote-only button.
    let toast = until("an OS toast", async || {
        env.toasts.lock().unwrap().first().cloned()
    })
    .await;
    assert!(
        toast
            .body
            .ends_with("Answer in BrowserMCP: Skip row / Stop run")
    );

    // "Open in browser" leaves the question open and says so on Telegram.
    env.telegram
        .press(1, CHAT, 101, &button_data(&send, "Open in browser"));
    let edit = env.telegram.wait_call("editMessageText", 1).await.remove(0);
    assert!(
        edit["text"]
            .as_str()
            .unwrap()
            .ends_with("Waiting for you in BrowserMCP.")
    );
    assert_eq!(
        button_labels(&edit),
        ["Skip row", "Stop run", "Open in browser"]
    );
    assert!(env.run_calls().is_empty());
    assert_eq!(env.service.open_questions().len(), 1);

    // "Skip row" → run.resume {id: r1}.
    env.telegram
        .press(2, CHAT, 101, &button_data(&send, "Skip row"));
    let closed = env.event("ui.notification_closed").await;
    assert_eq!(closed["choice"], "skip");
    until("a run command", async || {
        (!env.run_calls().is_empty()).then_some(())
    })
    .await;
    assert_eq!(env.run_calls(), ["run.resume r1"]);
}

#[tokio::test]
async fn heal_question_closes_when_resolved_elsewhere_and_ui_stop_cancels() {
    let mut env = env(|c, _| c.channels.telegram.enabled = false).await;
    env.events
        .emit("run.heal_needed", json!({"id": "r1", "row": 1}));
    let first = env.event("ui.notification").await;
    // Someone resumed the run from the runs panel: the toast goes away.
    env.events.emit(
        "command.started",
        json!({"name": "run.resume", "args": {"id": "r1"}}),
    );
    let closed = env.event("ui.notification_closed").await;
    assert_eq!(closed["id"], first["id"]);
    assert_eq!(closed["via"], "elsewhere");

    // A new pause, answered with Stop from the UI → run.cancel.
    env.events
        .emit("run.heal_needed", json!({"id": "r2", "row": 3}));
    let second = env.event("ui.notification").await;
    env.call("ui.answer", json!({"id": second["id"], "choice": "stop"}))
        .await
        .unwrap()
        .unwrap();
    until("a run command", async || {
        (!env.run_calls().is_empty()).then_some(())
    })
    .await;
    assert_eq!(env.run_calls(), ["run.cancel r2"]);
}

impl Env {
    async fn wait_run_calls(&self, n: usize) -> Vec<String> {
        until(&format!("{n} run command(s)"), async || {
            let calls = self.run_calls();
            (calls.len() >= n).then_some(calls)
        })
        .await
    }
}

fn proposal_event(proposal: &str, attempt: u64) -> Value {
    json!({
        "id": "r1", "proposal_id": proposal, "row": 7, "script": "login",
        "diff": "@@ -14 +14 @@\n-act(\"button\", \"Sign in\")\n+act(\"button\", \"Log in\")",
        "patched_code": "// patched", "explanation": "The button is now called Log in.",
        "confidence": 0.82, "attempt": attempt, "max_attempts": 3,
        "checks": ["applies", "transpiles"], "mode": "ask",
    })
}

#[tokio::test]
async fn heal_proposed_buttons_accept_on_telegram() {
    let mut env = env(|_, _| {}).await;
    env.events
        .emit("run.heal_proposed", proposal_event("hp1", 1));

    // UI: Accept / Reject. Telegram: Show diff, Accept, Reject, Open in browser.
    let toast = env.event("ui.notification").await;
    assert_eq!(toast["title"], "Run r1: patch proposed");
    assert_eq!(
        toast["actions"],
        json!([{"id": "accept", "label": "Accept"}, {"id": "reject", "label": "Reject"}])
    );
    let send = env.telegram.wait_call("sendMessage", 1).await.remove(0);
    assert_eq!(
        send["text"],
        "[warning] Run r1: patch proposed\nRow 8 of login: The button is now called Log in.\n\
         confidence 82% · attempt 1/3 · checks: applies, transpiles"
    );
    assert_eq!(
        button_labels(&send),
        ["Show diff", "Accept", "Reject", "Open in browser"]
    );

    // "Show diff" sends the diff and leaves the question open.
    env.telegram
        .press(1, CHAT, 101, &button_data(&send, "Show diff"));
    let sends = env.telegram.wait_call("sendMessage", 2).await;
    assert!(
        sends[1]["text"]
            .as_str()
            .unwrap()
            .contains("+act(\"button\", \"Log in\")"),
        "{}",
        sends[1]
    );
    assert!(env.run_calls().is_empty());
    assert_eq!(env.service.open_questions().len(), 1);

    // "Accept" → run.heal_accept {proposal_id}, not run.heal_apply with the code.
    env.telegram
        .press(2, CHAT, 101, &button_data(&send, "Accept"));
    let closed = env.event("ui.notification_closed").await;
    assert_eq!(closed["choice"], "accept");
    assert_eq!(closed["via"], "telegram");
    assert_eq!(env.wait_run_calls(1).await, ["run.heal_accept hp1"]);
}

#[tokio::test]
async fn heal_proposals_supersede_and_close_with_the_run() {
    let mut env = env(|c, _| c.channels.telegram.enabled = false).await;
    env.events.emit(
        "run.heal_needed",
        json!({"id": "r1", "row": 7, "script": "login"}),
    );
    let needed = env.event("ui.notification").await;
    env.events
        .emit("run.heal_proposed", proposal_event("hp1", 1));
    let first = env.event("ui.notification").await;

    // Reject from the UI → run.heal_reject; the heal_needed question stays.
    env.call("ui.answer", json!({"id": first["id"], "choice": "reject"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(env.wait_run_calls(1).await, ["run.heal_reject hp1"]);
    assert_eq!(
        env.service.open_questions(),
        [needed["id"].as_str().unwrap()]
    );

    // The next attempt; a newer one replaces it.
    env.events
        .emit("run.heal_proposed", proposal_event("hp2", 2));
    let second = env.event("ui.notification").await;
    env.events
        .emit("run.heal_proposed", proposal_event("hp3", 3));
    let closed = env.event("ui.notification_closed").await;
    assert_eq!(closed["id"], second["id"]);
    assert_eq!(closed["via"], "superseded");
    let third = env.event("ui.notification").await;
    assert_eq!(env.service.open_questions().len(), 2);

    // Accepted elsewhere (the Runs panel): the patch is applied, both questions go away.
    env.events.emit(
        "run.heal_applied",
        json!({"id": "r1", "proposal_id": "hp3", "script": "login.v2", "requeued": 1, "auto": false}),
    );
    let mut gone = vec![];
    for _ in 0..2 {
        let closed = env.event("ui.notification_closed").await;
        assert_eq!(closed["via"], "elsewhere");
        gone.push(closed["id"].as_str().unwrap().to_string());
    }
    gone.sort();
    let mut want = vec![
        needed["id"].as_str().unwrap().to_string(),
        third["id"].as_str().unwrap().to_string(),
    ];
    want.sort();
    assert_eq!(gone, want);
    assert!(env.service.open_questions().is_empty());
    // "Run r1 patched" is shown in the UI.
    let toast = env.event("ui.notification").await;
    assert_eq!(toast["title"], "Run r1 patched");
}

#[tokio::test]
async fn row_failures_are_aggregated() {
    let mut env = env(|_, _| {}).await;
    for row in 0..200 {
        env.events.emit(
            "run.row_failed",
            json!({"id": "r9", "row": row, "error": {"code": "TIMEOUT", "message": "slow"}}),
        );
    }
    env.events.emit(
        "run.finished",
        json!({"id": "r9", "status": "finished", "total": 200,
                                           "ok": 0, "failed": 200, "duration_ms": 1000}),
    );
    // Two toasts: the first failure and the finish (the other 199 failures are counted).
    let mut titles = vec![
        env.event("ui.notification").await["title"].clone(),
        env.event("ui.notification").await["title"].clone(),
    ];
    titles.sort_by_key(|t| t.to_string());
    assert_eq!(
        titles,
        [json!("Run r9 finished"), json!("Run r9: row 0 failed")]
    );
    // Only the finish went to Telegram (row failures: UI only by default).
    let sends = env.telegram.wait_call("sendMessage", 1).await;
    assert_eq!(sends.len(), 1);
    assert!(
        sends[0]["text"]
            .as_str()
            .unwrap()
            .contains("Run r9 finished")
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(env.telegram.calls("sendMessage").len(), 1);
}

#[tokio::test]
async fn webhook_and_focus_rules() {
    let mut env = env(|c, uri| {
        c.channels.telegram.enabled = false;
        c.channels.webhook.enabled = true;
        c.channels.webhook.url = format!("{uri}/hook");
    })
    .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&env.server)
        .await;

    env.events.emit(
        "trigger.fired",
        json!({"id": "t1", "script": "daily", "ok": false, "error": "boom"}),
    );
    env.event("ui.notification").await;
    let hook = until("the webhook call", async || {
        let requests = env.server.received_requests().await.unwrap();
        requests
            .iter()
            .find(|r| r.url.path() == "/hook")
            .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
    })
    .await;
    assert_eq!(hook["title"], "Trigger t1 failed (daily)");
    assert_eq!(hook["topic"], "trigger.fired");
    assert_eq!(hook["data"]["script"], "daily");
    assert_eq!(hook["level"], "error");

    // ai.done: the window is focused in this env → no notification at all.
    env.events
        .emit("ai.done", json!({"session": "a1", "reason": "end_turn"}));
    env.events
        .emit("download.done", json!({"path": "C:/x.zip"}));
    let next = env.event("ui.notification").await;
    assert_eq!(next["title"], "Download finished");
}

#[tokio::test]
async fn config_commands_round_trip_and_test_channel() {
    let env = env(|_, _| {}).await;
    let got = env
        .call("notify.config_get", json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["channels"]["telegram"]["bot_token"], "…OKEN");
    assert_eq!(got["channels"]["ui"], true);

    let set = env
        .call(
            "notify.config_set",
            json!({"os": false, "telegram": {"bot_token": "…OKEN", "chat_id": "778"}}),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(set["channels"]["os"], false);
    assert_eq!(set["channels"]["telegram"]["chat_id"], "778");
    let stored = env.service.config();
    assert_eq!(
        stored.channels.telegram.bot_token, TOKEN,
        "masked token kept"
    );

    let test = env
        .call("notify.test", json!({"channel": "telegram"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        test,
        json!({"channel": "telegram", "ok": true, "error": null})
    );
    let send = env.telegram.wait_call("sendMessage", 1).await.remove(0);
    assert_eq!(send["chat_id"], 778);
    assert_eq!(
        send["text"],
        "BrowserMCP test\nNotifications to telegram work."
    );

    let sent = env
        .call(
            "notify.send",
            json!({"title": "Hello", "body": "there", "level": "success"}),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent["delivered"], json!(["ui", "telegram"]));
    let n = env
        .call("ui.notify", json!({"text": "Done"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n["delivered"], json!(["ui"]));
}

#[tokio::test]
async fn telegram_errors_are_reported_without_the_token() {
    let env = env(|c, _| c.channels.telegram.api_base = "http://127.0.0.1:9".into()).await;
    let test = env
        .call("notify.test", json!({"channel": "telegram"}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(test["ok"], false);
    let error = test["error"].as_str().unwrap();
    assert!(error.starts_with("telegram sendMessage"), "{error}");
    assert!(!error.contains("TEST-TOKEN"), "{error}");
}
