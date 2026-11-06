//! JavaScript dialogs: watching for them, the auto-handling policy and the guard.
//!
//! Chromium reports dialogs as CDP events once `Page.enable` is on:
//! `Page.javascriptDialogOpening {type, message, defaultPrompt, url}` and
//! `Page.javascriptDialogClosed {result, userInput}`; `Page.handleJavaScriptDialog` answers
//! one. One background task per tab ([`ensure_watched`]) reads these events and
//!
//! * emits `page.dialog_opened` / `page.dialog_closed` on the event bus;
//! * applies the policy ([`decide`]): accept, dismiss, or leave it open ("ask");
//! * remembers dialogs left open, so the [`DialogGuard`] can stop other page commands with
//!   a `DIALOG_OPEN` error that describes the dialog, instead of letting them hang.
//!
//! "Automation is active" (used by the `auto` rule) means: a caller other than the
//! browser's own UI / keyboard (MCP, REST, WS, scripts, runs, the built-in agent) ran a
//! command within the last [`AUTOMATION_WINDOW`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bmcp_core::{
    BoxFuture, BrowserBackend, Caller, CdpEvents, CmdError, CommandGuard, Ctx, ErrorCode,
    EventBus, TabId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

/// How long after the last API call automation still counts as active.
pub const AUTOMATION_WINDOW: Duration = Duration::from_secs(60);

/// Event topics this module emits.
pub mod topics {
    /// `{tab, type, message, default_prompt, url, handling: "accept"|"dismiss"|"ask"}`
    pub const DIALOG_OPENED: &str = "page.dialog_opened";
    /// `{tab, accepted, user_input}`
    pub const DIALOG_CLOSED: &str = "page.dialog_closed";
    /// `{tab, error}`: the policy said accept/dismiss but answering failed (left open).
    pub const DIALOG_ERROR: &str = "page.dialog_error";
}

/// The four kinds of JavaScript dialogs (CDP `DialogType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DialogType {
    Alert,
    Confirm,
    Prompt,
    /// "Leave site? Changes you made may not be saved."
    Beforeunload,
}

impl DialogType {
    fn from_cdp(text: &str) -> Option<Self> {
        Some(match text {
            "alert" => DialogType::Alert,
            "confirm" => DialogType::Confirm,
            "prompt" => DialogType::Prompt,
            "beforeunload" => DialogType::Beforeunload,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DialogType::Alert => "alert",
            DialogType::Confirm => "confirm",
            DialogType::Prompt => "prompt",
            DialogType::Beforeunload => "beforeunload",
        }
    }
}

/// What to do with one kind of dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// Press OK at once (prompts get `prompt_text`, else their default text).
    Accept,
    /// Press Cancel at once.
    Dismiss,
    /// Leave it open for the agent or the human (event + `DIALOG_OPEN` errors).
    Ask,
    /// `accept` while automation is active, otherwise `ask`.
    Auto,
}

/// The outcome of [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Dismiss,
    Leave,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Decision::Accept => "accept",
            Decision::Dismiss => "dismiss",
            Decision::Leave => "ask",
        }
    }
}

/// The policy table: one rule + whether automation is active → what happens.
pub fn decide(rule: Rule, automation: bool) -> Decision {
    match rule {
        Rule::Accept => Decision::Accept,
        Rule::Dismiss => Decision::Dismiss,
        Rule::Ask => Decision::Leave,
        Rule::Auto if automation => Decision::Accept,
        Rule::Auto => Decision::Leave,
    }
}

/// A (partial) policy. `None` fields fall back to the next level: tab → global → defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Policy {
    pub alert: Option<Rule>,
    pub confirm: Option<Rule>,
    pub prompt: Option<Rule>,
    pub beforeunload: Option<Rule>,
    /// Text typed into prompts that are accepted automatically (default: their default text).
    pub prompt_text: Option<String>,
}

impl Policy {
    /// The built-in defaults: leave dialogs to the agent/human, but accept `beforeunload`
    /// while automation runs (so closing/navigating tabs does not get stuck).
    pub fn defaults() -> Policy {
        Policy {
            alert: Some(Rule::Ask),
            confirm: Some(Rule::Ask),
            prompt: Some(Rule::Ask),
            beforeunload: Some(Rule::Auto),
            prompt_text: None,
        }
    }

    /// The rule for one kind (`Ask` when unset, which only happens for partial policies).
    pub fn rule(&self, kind: DialogType) -> Rule {
        match kind {
            DialogType::Alert => self.alert,
            DialogType::Confirm => self.confirm,
            DialogType::Prompt => self.prompt,
            DialogType::Beforeunload => self.beforeunload,
        }
        .unwrap_or(Rule::Ask)
    }

    /// `self` with its unset fields taken from `base`.
    pub fn over(&self, base: &Policy) -> Policy {
        Policy {
            alert: self.alert.or(base.alert),
            confirm: self.confirm.or(base.confirm),
            prompt: self.prompt.or(base.prompt),
            beforeunload: self.beforeunload.or(base.beforeunload),
            prompt_text: self.prompt_text.clone().or_else(|| base.prompt_text.clone()),
        }
    }

    /// Copy the fields that are set in `changes` into `self`.
    pub fn apply(&mut self, changes: &Policy) {
        *self = changes.over(self);
    }
}

/// A dialog that is open and waiting for an answer.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OpenDialog {
    pub tab: TabId,
    #[serde(rename = "type")]
    pub kind: DialogType,
    pub message: String,
    /// Prefilled text of a prompt ("" for the other kinds).
    pub default_prompt: String,
    /// URL of the frame that opened it.
    pub url: String,
}

/// Shared dialog state of the whole browser (one in the registry's `Extensions`).
pub struct DialogState {
    inner: Mutex<Inner>,
    /// Bumped whenever the set of open dialogs changes; the guards wait on it.
    changed: tokio::sync::watch::Sender<u64>,
}

#[derive(Default)]
struct Inner {
    /// Dialogs left open, by tab.
    open: HashMap<TabId, OpenDialog>,
    global: Policy,
    tabs: HashMap<TabId, Policy>,
    /// Tabs with a running event listener (and `Page.enable` sent).
    watched: HashSet<TabId>,
    /// Last command from a non-UI caller.
    last_automation: Option<Instant>,
}

impl Default for DialogState {
    fn default() -> Self {
        DialogState {
            inner: Mutex::default(),
            changed: tokio::sync::watch::channel(0).0,
        }
    }
}

impl DialogState {
    /// The shared instance from a call's context (created on first use).
    pub fn from_ctx(ctx: &Ctx) -> Arc<DialogState> {
        ctx.extensions.get_or_insert_with(DialogState::default)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock cannot leave the maps half-updated.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The dialog left open on `tab`, if any.
    pub fn open_on(&self, tab: TabId) -> Option<OpenDialog> {
        self.lock().open.get(&tab).cloned()
    }

    fn any_open(&self) -> bool {
        !self.lock().open.is_empty()
    }

    fn set_open(&self, dialog: OpenDialog) {
        self.lock().open.insert(dialog.tab, dialog);
        self.changed.send_modify(|n| *n += 1);
    }

    /// Forget the open dialog of `tab` (answered or gone) and return it.
    pub fn take_open(&self, tab: TabId) -> Option<OpenDialog> {
        let taken = self.lock().open.remove(&tab);
        if taken.is_some() {
            self.changed.send_modify(|n| *n += 1);
        }
        taken
    }

    /// The policy in force on `tab`: tab rules over global rules over the defaults.
    pub fn effective(&self, tab: Option<TabId>) -> Policy {
        let inner = self.lock();
        let global = inner.global.over(&Policy::defaults());
        match tab.and_then(|t| inner.tabs.get(&t)) {
            Some(tab_policy) => tab_policy.over(&global),
            None => global,
        }
    }

    /// Change the global policy (`tab: None`) or a tab's own rules. `reset` first removes
    /// that level's rules.
    pub fn set_policy(&self, tab: Option<TabId>, changes: &Policy, reset: bool) {
        let mut inner = self.lock();
        let level = match tab {
            Some(tab) => inner.tabs.entry(tab).or_default(),
            None => &mut inner.global,
        };
        if reset {
            *level = Policy::default();
        }
        level.apply(changes);
    }

    /// Remember that automation just did something (see the module docs).
    pub fn note_automation(&self) {
        self.lock().last_automation = Some(Instant::now());
    }

    pub fn automation_active(&self) -> bool {
        self.lock()
            .last_automation
            .is_some_and(|at| at.elapsed() < AUTOMATION_WINDOW)
    }

    /// Mark `tab` as watched; false when it already was.
    fn start_watching(&self, tab: TabId) -> bool {
        self.lock().watched.insert(tab)
    }

    /// The tab is gone (or watching it failed): forget everything about it.
    fn forget(&self, tab: TabId) {
        let mut inner = self.lock();
        inner.watched.remove(&tab);
        inner.tabs.remove(&tab);
        let had_dialog = inner.open.remove(&tab).is_some();
        drop(inner);
        if had_dialog {
            self.changed.send_modify(|n| *n += 1);
        }
    }
}

// ------------------------------------------------------------------ listener

/// Make sure `tab` has a dialog listener and `Page.enable` (once per tab; cheap afterwards).
pub async fn ensure_watched(
    state: &Arc<DialogState>,
    backend: &Arc<dyn BrowserBackend>,
    events: &EventBus,
    tab: TabId,
) -> Result<(), CmdError> {
    if !state.start_watching(tab) {
        return Ok(());
    }
    // Subscribe before enabling, so no event between the two is lost.
    let setup = async {
        let rx = backend.cdp_events(tab).await?;
        backend
            .cdp(tab, "Page.enable".into(), json!({}))
            .await
            .map(|_| rx)
    };
    match setup.await {
        Ok(rx) => {
            tokio::spawn(listen(
                state.clone(),
                backend.clone(),
                events.clone(),
                tab,
                rx,
            ));
            Ok(())
        }
        Err(err) => {
            state.forget(tab);
            Err(err.into())
        }
    }
}

/// [`ensure_watched`] with the call's context.
pub async fn ensure_watched_ctx(ctx: &Ctx, tab: TabId) -> Result<(), CmdError> {
    ensure_watched(&DialogState::from_ctx(ctx), &ctx.backend, &ctx.events, tab).await
}

/// Reads one tab's CDP events until the tab closes.
async fn listen(
    state: Arc<DialogState>,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
    tab: TabId,
    mut rx: CdpEvents,
) {
    loop {
        match rx.recv().await {
            // Only the page target itself (dialogs of iframes are reported there too).
            Ok(event) if event.session_id.is_none() => match event.method.as_str() {
                "Page.javascriptDialogOpening" => {
                    on_opening(&state, &backend, &events, tab, &event.params);
                }
                "Page.javascriptDialogClosed" => {
                    state.take_open(tab);
                    events.emit(
                        topics::DIALOG_CLOSED,
                        json!({
                            "tab": tab,
                            "accepted": event.params["result"].as_bool().unwrap_or(false),
                            "user_input": event.params["userInput"].as_str().unwrap_or_default(),
                        }),
                    );
                }
                _ => {}
            },
            Ok(_) | Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => break,
        }
    }
    state.forget(tab);
}

/// A dialog opened: report it, then answer it or leave it open, as the policy says.
fn on_opening(
    state: &Arc<DialogState>,
    backend: &Arc<dyn BrowserBackend>,
    events: &EventBus,
    tab: TabId,
    params: &Value,
) {
    let Some(kind) = params["type"].as_str().and_then(DialogType::from_cdp) else {
        return;
    };
    let text = |key: &str| params[key].as_str().unwrap_or_default().to_string();
    let dialog = OpenDialog {
        tab,
        kind,
        message: text("message"),
        default_prompt: text("defaultPrompt"),
        url: text("url"),
    };
    let policy = state.effective(Some(tab));
    let decision = decide(policy.rule(kind), state.automation_active());
    let mut data = json!(dialog);
    data["handling"] = json!(decision.as_str());
    events.emit(topics::DIALOG_OPENED, data);

    let accept = match decision {
        Decision::Leave => return state.set_open(dialog),
        Decision::Accept => true,
        Decision::Dismiss => false,
    };
    let mut answer = json!({ "accept": accept });
    if accept && kind == DialogType::Prompt {
        let text = policy
            .prompt_text
            .unwrap_or_else(|| dialog.default_prompt.clone());
        answer["promptText"] = json!(text);
    }
    // Answer from a task: the listener keeps reading events meanwhile.
    let (state, backend, events) = (state.clone(), backend.clone(), events.clone());
    tokio::spawn(async move {
        let reply = backend
            .cdp(tab, "Page.handleJavaScriptDialog".into(), answer)
            .await;
        if let Err(err) = reply {
            // Could not answer: treat it as left open, so commands get DIALOG_OPEN.
            tracing::warn!(%err, %tab, "auto-handling a dialog failed");
            events.emit(
                topics::DIALOG_ERROR,
                json!({ "tab": tab, "error": err.to_string() }),
            );
            state.set_open(dialog);
        }
    });
}

/// Watch every tab: the ones open now and each new one (`tab.created`).
pub fn spawn_tab_watcher(
    state: Arc<DialogState>,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
) {
    // Subscribe first, so a tab created while we list the existing ones is not missed.
    let mut bus = events.subscribe();
    tokio::spawn(async move {
        if let Ok(tabs) = backend.list_tabs().await {
            for tab in tabs {
                if let Err(err) = ensure_watched(&state, &backend, &events, tab.id).await {
                    tracing::debug!(%err, tab = %tab.id, "dialog watcher not started");
                }
            }
        }
        loop {
            match bus.recv().await {
                Ok(event) if event.topic == bmcp_core::events::topics::TAB_CREATED => {
                    let Some(id) = event.data["tab"]["id"].as_i64() else {
                        continue;
                    };
                    let tab = TabId(id as i32);
                    if let Err(err) = ensure_watched(&state, &backend, &events, tab).await {
                        tracing::debug!(%err, %tab, "dialog watcher not started");
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

// ------------------------------------------------------------------ guard

/// Stops `page.*`, `input.*` and `nav.*` commands on a tab with an open dialog (they would
/// hang otherwise) and notes automation activity for the `auto` rule.
pub struct DialogGuard(pub Arc<DialogState>);

/// Commands the guard watches: everything that talks to the page, except answering dialogs.
pub fn is_guarded(name: &str) -> bool {
    let page_command = ["page.", "input.", "nav."]
        .iter()
        .any(|prefix| name.starts_with(prefix));
    page_command && !matches!(name, "page.dialog" | "page.dialog_policy")
}

impl CommandGuard for DialogGuard {
    fn watch(
        &self,
        ctx: &Ctx,
        name: &'static str,
        args: &Value,
    ) -> Option<BoxFuture<'static, CmdError>> {
        if !matches!(ctx.caller, Caller::Ui | Caller::Keymap) {
            self.0.note_automation();
        }
        if !is_guarded(name) {
            return None;
        }
        let state = self.0.clone();
        let backend = ctx.backend.clone();
        let explicit = args.get("tab").and_then(Value::as_i64).map(|t| TabId(t as i32));
        Some(Box::pin(async move {
            // Subscribe before looking, so a dialog opening in between is not missed.
            let mut changes = state.changed.subscribe();
            let mut first_look = true;
            loop {
                // Fast path: no dialog anywhere → nothing to resolve.
                if state.any_open() {
                    let tab = match explicit {
                        Some(tab) => Some(tab),
                        // Same default as `Ctx::resolve_tab`: the newest tab.
                        None => backend
                            .list_tabs()
                            .await
                            .ok()
                            .and_then(|tabs| tabs.iter().map(|t| t.id).max()),
                    };
                    if let Some(dialog) = tab.and_then(|tab| state.open_on(tab)) {
                        return dialog_open_error(name, &dialog, !first_look);
                    }
                }
                first_look = false;
                if changes.changed().await.is_err() {
                    // The state is gone (browser shutting down): never fire.
                    std::future::pending::<()>().await;
                }
            }
        }))
    }
}

/// The `DIALOG_OPEN` error for `command`. `while_running`: the dialog opened after the
/// command had started (so its action most likely happened, e.g. the click that opened it).
pub fn dialog_open_error(command: &str, dialog: &OpenDialog, while_running: bool) -> CmdError {
    let what = format!(
        "a {} dialog {} on tab {}: \"{}\"",
        dialog.kind.as_str(),
        if while_running { "opened" } else { "is open" },
        dialog.tab,
        dialog.message
    );
    let message = if while_running {
        format!("{command} stopped: {what} (the action probably happened)")
    } else {
        format!("{command} is blocked: {what}")
    };
    CmdError::new(ErrorCode::DialogOpen, message)
        .with_hint(
            "Answer it with page_dialog {\"action\":\"accept\"} or {\"action\":\"dismiss\"} \
             (a prompt also takes \"text\"), then go on. page_dialog_policy can answer such \
             dialogs automatically.",
        )
        .with_candidates(vec![json!(dialog)])
}
