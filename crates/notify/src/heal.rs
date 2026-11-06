//! The self-healing question: `run.heal_needed` → buttons → run commands.
//!
//! `bmcp-jobs` pauses a run when a row breaks on a page selector and emits
//! `run.heal_needed {id, row, script, error, step, line}`. We turn it into a question:
//!
//! ```text
//! Run r12 needs a fix
//! Row 7 failed at step page.act (line 14): NOT_FOUND: no element "Sign in"
//! [Skip row] [Stop run] [Open in browser]
//! ```
//!
//! * **Skip row** → `run.resume {id}` (the broken rows count as failed, the run goes on);
//! * **Stop run** → `run.cancel {id}`;
//! * **Open in browser** (Telegram only) → nothing is run; the question stays open in the UI;
//! * when the event carries a proposed fix (`patched_code`, from a future AI patcher) there are
//!   also **Apply patch** → `run.heal_apply {id, patched_code}` and **Show patch** (Telegram:
//!   sends the code as a message).

use serde_json::{Value, json};

use crate::model::{Action, truncate};
use crate::rules::error_text;

pub const SKIP: &str = "skip";
pub const STOP: &str = "stop";
pub const OPEN: &str = "open";
pub const APPLY: &str = "apply";
pub const SHOW: &str = "show";

/// "Row 7 failed at step page.act (line 14): NOT_FOUND: …\nScript: login"
pub fn describe(data: &Value) -> String {
    let row = data.get("row").map(Value::to_string).unwrap_or_default();
    let mut text = format!("Row {row} failed");
    if let Some(step) = data.get("step").and_then(Value::as_str) {
        text.push_str(&format!(" at step {step}"));
    }
    if let Some(line) = data.get("line").and_then(Value::as_u64) {
        text.push_str(&format!(" (line {line})"));
    }
    let error = error_text(data.get("error").unwrap_or(&Value::Null));
    if !error.is_empty() {
        text.push_str(&format!(": {error}"));
    }
    if let Some(script) = data.get("script").and_then(Value::as_str) {
        text.push_str(&format!("\nScript: {script}"));
    }
    text
}

/// The proposed fix carried by the event, if any.
pub fn patch(data: &Value) -> Option<&str> {
    data.get("patched_code")
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
}

pub fn actions(data: &Value) -> Vec<Action> {
    let mut actions = Vec::new();
    if patch(data).is_some() {
        actions.push(Action::new(SHOW, "Show patch").remote_only());
        actions.push(Action::new(APPLY, "Apply patch"));
    }
    actions.push(Action::new(SKIP, "Skip row"));
    actions.push(Action::new(STOP, "Stop run"));
    actions.push(Action::new(OPEN, "Open in browser").remote_only());
    actions
}

/// Buttons that do not answer the question (it stays open).
pub fn is_final(action: &str) -> bool {
    !matches!(action, OPEN | SHOW)
}

/// The run command for a final answer: `(name, args)`.
pub fn command_for(action: &str, run: &str, patched_code: Option<&str>) -> Option<(String, Value)> {
    match action {
        SKIP => Some(("run.resume".into(), json!({ "id": run }))),
        STOP => Some(("run.cancel".into(), json!({ "id": run }))),
        APPLY => patched_code.map(|code| {
            (
                "run.heal_apply".into(),
                json!({ "id": run, "patched_code": code }),
            )
        }),
        _ => None,
    }
}

/// The patch as a Telegram message.
pub fn patch_message(run: &str, code: &str) -> String {
    truncate(&format!("Proposed patch for run {run}:\n\n{code}"), 3900)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heal_text_and_buttons() {
        let data = json!({
            "id": "r1", "row": 7, "script": "login", "step": "page.act", "line": 14,
            "error": {"code": "NOT_FOUND", "message": "no Sign in button"},
        });
        assert_eq!(
            describe(&data),
            "Row 7 failed at step page.act (line 14): NOT_FOUND: no Sign in button\nScript: login"
        );
        let ids: Vec<_> = actions(&data).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, [SKIP, STOP, OPEN]);

        let mut with_patch = data.clone();
        with_patch["patched_code"] = json!("browser.page.act(...)");
        let ids: Vec<_> = actions(&with_patch).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, [SHOW, APPLY, SKIP, STOP, OPEN]);
    }

    #[test]
    fn buttons_map_to_run_commands() {
        assert_eq!(
            command_for(SKIP, "r1", None),
            Some(("run.resume".into(), json!({"id": "r1"})))
        );
        assert_eq!(
            command_for(STOP, "r1", None),
            Some(("run.cancel".into(), json!({"id": "r1"})))
        );
        assert_eq!(command_for(APPLY, "r1", None), None, "no patch, no apply");
        assert_eq!(
            command_for(APPLY, "r1", Some("code")),
            Some((
                "run.heal_apply".into(),
                json!({"id": "r1", "patched_code": "code"})
            ))
        );
        assert_eq!(command_for(OPEN, "r1", None), None);
        assert!(!is_final(OPEN) && !is_final(SHOW) && is_final(SKIP));
    }
}
