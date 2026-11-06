//! BrowserMCP page tools: the "files and dialogs" part of the plan.
//!
//! * `page.dialog` / `page.dialog_policy` — answer JavaScript dialogs (alert, confirm, prompt,
//!   beforeunload) and handle them automatically by policy ([`dialog`]);
//! * `page.upload` — put files into an `<input type=file>`, or into the chooser a button
//!   opens ([`upload`]); paths may be relative to a run folder `data/runs/<id>/`;
//! * `page.pdf` / `page.mhtml` — save the page as PDF (data/pdf/) or MHTML (data/mhtml/)
//!   ([`print`]);
//! * `page.find_text` / `page.find_clear` — Ctrl+F: highlight matches and scroll to the
//!   current one ([`find`]).
//!
//! # How agents see dialogs
//!
//! A JavaScript dialog freezes the page: every CDP call into it waits until the dialog is
//! answered. So that agents do not just hang, this crate:
//!
//! 1. emits `page.dialog_opened {tab, type, message, default_prompt, url, handling}` on the
//!    event bus (WS clients: `events.subscribe {"topics":["page.*"]}`), and
//!    `page.dialog_closed {tab, accepted, user_input}` when it is gone;
//! 2. answers it at once when the tab's policy says so (`page.dialog_policy`; by default
//!    only `beforeunload` is answered, and only while automation is active);
//! 3. otherwise leaves it open for the agent or the human, and every `page.*` / `input.*` /
//!    `nav.*` command on that tab fails right away (or, if it was already running, as soon
//!    as the dialog opens) with
//!
//!    ```json
//!    {"code":"DIALOG_OPEN","retry":"no",
//!     "message":"page.act stopped: a confirm dialog opened on tab 3: \"Delete?\" (the action probably happened)",
//!     "hint":"Answer it with page_dialog {\"action\":\"accept\"} or {\"action\":\"dismiss\"} …",
//!     "candidates":[{"tab":3,"type":"confirm","message":"Delete?","default_prompt":"","url":"https://…"}]}
//!    ```
//!
//!    The agent answers with `page_dialog` and goes on.
//!
//! Everything goes through `BrowserBackend::cdp` / `cdp_events` (no CEF dependency); the
//! tests use `bmcp_core::testing::FakeBackend`. Stealth: no `Runtime.enable`; page scripts
//! (find in page) run in an isolated world. `Page.enable` is on for every tab (dialog events).

pub mod commands;
pub mod dialog;
pub mod find;
pub mod print;
pub mod upload;
mod util;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use bmcp_core::{BrowserBackend, EventBus, Registry};

pub use dialog::{DialogGuard, DialogState};

/// Register the commands, their shared state and the dialog guard.
pub fn register(registry: &mut Registry) {
    let dialogs = registry.extensions().insert(DialogState::default());
    registry.extensions().insert(find::FindState::default());
    registry.add_guard(Arc::new(DialogGuard(dialogs)));
    registry
        .add::<commands::Dialog>()
        .add::<commands::DialogPolicy>()
        .add::<commands::Upload>()
        .add::<commands::Pdf>()
        .add::<commands::Mhtml>()
        .add::<commands::FindText>()
        .add::<commands::FindClear>();
}

/// Start watching every tab for dialogs (existing tabs and each `tab.created`), so dialogs
/// are reported and auto-handled even before any page tool was called. Call once, inside a
/// tokio runtime, after [`register`].
pub fn start(registry: &Registry, backend: Arc<dyn BrowserBackend>, events: EventBus) {
    let state = registry
        .extensions()
        .get_or_insert_with(DialogState::default);
    dialog::spawn_tab_watcher(state, backend, events);
}
