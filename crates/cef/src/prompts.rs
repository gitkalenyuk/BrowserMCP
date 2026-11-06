//! Permission prompts of web pages (geolocation, notifications, clipboard, camera/mic, …).
//!
//! Alloy has no prompt UI of its own. A request is answered at once when the stored rules
//! decide it (`permissions.set`; `bmcp_profiles::permissions::decision`). Otherwise, for a tab
//! in a window, the request waits here and the browser UI shows its permission bar:
//! `permission.prompt {id, tab, origin, kinds}` on the bus, answered with
//! `permissions.answer {id, allow, remember}` ([`answer`]). `permission.prompt_closed {id}`
//! tells the UI to hide it (answered, dismissed by the page, or the tab closed). Off-screen
//! tabs (automation) never wait: no rule = deny, as before.
//!
//! UI thread only.

use std::collections::BTreeMap;

use bmcp_core::TabId;
use bmcp_profiles::PermissionPrompt;
use bmcp_profiles::permissions::{Decision, PermissionKind};
use cef::*;
use serde_json::json;

use crate::bridge::{emit, with_ui};

/// Topics of the permission bar.
pub(crate) const PROMPT: &str = "permission.prompt";
pub(crate) const PROMPT_CLOSED: &str = "permission.prompt_closed";

/// CEF's way to answer a request.
pub(crate) enum Answer {
    /// `OnShowPermissionPrompt`.
    Prompt(PermissionPromptCallback),
    /// `OnRequestMediaAccessPermission` with the requested bits.
    Media(MediaAccessCallback, u32),
}

/// A request waiting for a person.
pub(crate) struct Waiting {
    pub prompt: PermissionPrompt,
    pub answer: Answer,
}

/// Ids for media requests (CEF gives them none); far above CEF's prompt ids.
fn next_media_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1 << 40);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// What to do with a request of `kinds` from `origin` in tab `browser`: `Some(answer)` to
/// answer now, `None` = it waits for the permission bar (then call [`wait`]).
pub(crate) fn decide(browser: &Browser, origin: &str, kinds: &[PermissionKind]) -> Option<bool> {
    let Some(store) = crate::permission_store() else {
        return Some(false);
    };
    let id = TabId(browser.identifier());
    // Only a tab in a window has someone to ask.
    let in_window = with_ui(|ui| {
        ui.tabs
            .get(&id)
            .is_some_and(|t| !t.offscreen && t.window.is_some())
    });
    match store.decision(origin, kinds) {
        Decision::Allow => Some(true),
        Decision::Deny => Some(false),
        Decision::Ask if in_window => None,
        Decision::Ask => Some(false),
    }
}

/// Keep `answer` until a person decides; show the bar. `prompt_id` = CEF's id (None for
/// media requests).
pub(crate) fn wait(
    browser: &Browser,
    prompt_id: Option<u64>,
    origin: &str,
    kinds: Vec<PermissionKind>,
    answer: Answer,
) {
    let id = prompt_id.unwrap_or_else(next_media_id);
    let prompt = PermissionPrompt {
        id,
        tab: TabId(browser.identifier()),
        origin: bmcp_profiles::permissions::normalize_origin(origin),
        kinds,
    };
    let payload = serde_json::to_value(&prompt).unwrap_or_default();
    with_ui(|ui| ui.prompts.insert(id, Waiting { prompt, answer }));
    tracing::info!(id, %payload, "permission request waits for the permission bar");
    emit(PROMPT, payload);
}

/// Give CEF the answer.
fn send(answer: Answer, allow: bool) {
    match answer {
        Answer::Prompt(callback) => callback.cont(if allow {
            PermissionRequestResult::ACCEPT
        } else {
            PermissionRequestResult::DENY
        }),
        Answer::Media(callback, bits) => callback.cont(if allow { bits } else { 0 }),
    }
}

/// `permissions.answer`. Returns the prompt, `None` if it is gone.
pub(crate) fn answer(id: u64, allow: bool) -> Option<PermissionPrompt> {
    let waiting = with_ui(|ui| ui.prompts.remove(&id))?;
    tracing::info!(
        id,
        allow,
        origin = waiting.prompt.origin,
        "permission answered"
    );
    send(waiting.answer, allow);
    emit(PROMPT_CLOSED, json!({ "id": id, "allow": allow }));
    Some(waiting.prompt)
}

/// `permissions.pending`.
pub(crate) fn pending() -> Vec<PermissionPrompt> {
    with_ui(|ui| ui.prompts.values().map(|w| w.prompt.clone()).collect())
}

/// CEF dismissed a prompt itself (navigation, the page went away).
pub(crate) fn dismissed(prompt_id: u64) {
    let removed = with_ui(|ui| ui.prompts.remove(&prompt_id));
    if removed.is_some() {
        emit(PROMPT_CLOSED, json!({ "id": prompt_id, "allow": null }));
    }
    drop(removed);
}

/// A tab closed or navigated: its waiting requests are denied and the bar goes away.
pub(crate) fn drop_for_tab(tab: TabId) {
    let gone: BTreeMap<u64, Waiting> = with_ui(|ui| {
        let ids: Vec<u64> = ui
            .prompts
            .iter()
            .filter(|(_, w)| w.prompt.tab == tab)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| ui.prompts.remove(&id).map(|w| (id, w)))
            .collect()
    });
    for (id, waiting) in gone {
        send(waiting.answer, false);
        emit(PROMPT_CLOSED, json!({ "id": id, "allow": false }));
    }
}
