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
//!
//! `bmcp-heal` then emits `run.heal_proposed {id, proposal_id, diff, explanation, confidence,
//! attempt, max_attempts, checks, …}` with an AI patch. That is a second question:
//!
//! * **Accept** → `run.heal_accept {proposal_id}` (saves `<script>.vN`, re-queues the rows);
//! * **Reject** → `run.heal_reject {proposal_id}` (the AI tries again, up to `max_attempts`);
//! * **Show diff** (Telegram only) → sends the diff as a message; **Open in browser** as above.
//!
//! A newer proposal for the run replaces the older question; `run.heal_applied` closes every
//! heal question of the run, `run.heal_failed` the proposal.

use serde_json::{Value, json};

use crate::model::{Action, truncate};
use crate::rules::error_text;

pub const SKIP: &str = "skip";
pub const STOP: &str = "stop";
pub const OPEN: &str = "open";
pub const APPLY: &str = "apply";
pub const SHOW: &str = "show";
pub const ACCEPT: &str = "accept";
pub const REJECT: &str = "reject";

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

/// "Row 8 of login: the button is now called Log in\nconfidence 82% · attempt 1/3 · checks:
/// applies, transpiles"
pub fn describe_proposal(data: &Value) -> String {
    let row = data.get("row").and_then(Value::as_u64).map(|r| r + 1);
    let script = data.get("script").and_then(Value::as_str).unwrap_or("");
    let explanation = data
        .get("explanation")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut text = match row {
        Some(row) => format!("Row {row} of {script}"),
        None => format!("Script {script}"),
    };
    if !explanation.is_empty() {
        text.push_str(&format!(": {explanation}"));
    }
    let mut facts = Vec::new();
    if let Some(c) = data.get("confidence").and_then(Value::as_f64) {
        facts.push(format!("confidence {}%", (c * 100.0).round() as i64));
    }
    if let (Some(a), Some(m)) = (
        data.get("attempt").and_then(Value::as_u64),
        data.get("max_attempts").and_then(Value::as_u64),
    ) {
        facts.push(format!("attempt {a}/{m}"));
    }
    let checks: Vec<&str> = data
        .get("checks")
        .and_then(Value::as_array)
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !checks.is_empty() {
        facts.push(format!("checks: {}", checks.join(", ")));
    }
    if !facts.is_empty() {
        text.push('\n');
        text.push_str(&facts.join(" · "));
    }
    if let Some(note) = data
        .get("note")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
    {
        text.push_str(&format!("\nNot applied automatically: {note}"));
    }
    text
}

/// The proposal id and diff of a `run.heal_proposed` event.
pub fn proposal(data: &Value) -> Option<(&str, &str)> {
    let id = data.get("proposal_id").and_then(Value::as_str)?;
    let diff = data.get("diff").and_then(Value::as_str).unwrap_or("");
    Some((id, diff))
}

/// Buttons of a `run.heal_proposed` question.
pub fn proposal_actions() -> Vec<Action> {
    vec![
        Action::new(SHOW, "Show diff").remote_only(),
        Action::new(ACCEPT, "Accept"),
        Action::new(REJECT, "Reject"),
        Action::new(OPEN, "Open in browser").remote_only(),
    ]
}

/// Buttons that do not answer the question (it stays open).
pub fn is_final(action: &str) -> bool {
    !matches!(action, OPEN | SHOW)
}

/// The run command for a final answer: `(name, args)`. `proposal` is set for a
/// `run.heal_proposed` question (which never applies `patched_code` itself).
pub fn command_for(
    action: &str,
    run: &str,
    patched_code: Option<&str>,
    proposal: Option<&str>,
) -> Option<(String, Value)> {
    match (action, proposal) {
        (ACCEPT, Some(id)) => Some(("run.heal_accept".into(), json!({ "proposal_id": id }))),
        (REJECT, Some(id)) => Some(("run.heal_reject".into(), json!({ "proposal_id": id }))),
        (SKIP, _) => Some(("run.resume".into(), json!({ "id": run }))),
        (STOP, _) => Some(("run.cancel".into(), json!({ "id": run }))),
        (APPLY, None) => patched_code.map(|code| {
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
            command_for(SKIP, "r1", None, None),
            Some(("run.resume".into(), json!({"id": "r1"})))
        );
        assert_eq!(
            command_for(STOP, "r1", None, None),
            Some(("run.cancel".into(), json!({"id": "r1"})))
        );
        assert_eq!(
            command_for(APPLY, "r1", None, None),
            None,
            "no patch, no apply"
        );
        assert_eq!(
            command_for(APPLY, "r1", Some("code"), None),
            Some((
                "run.heal_apply".into(),
                json!({"id": "r1", "patched_code": "code"})
            ))
        );
        assert_eq!(command_for(OPEN, "r1", None, None), None);
        assert!(!is_final(OPEN) && !is_final(SHOW) && is_final(SKIP));
    }

    #[test]
    fn proposal_text_and_buttons() {
        let data = json!({
            "id": "r1", "proposal_id": "hp3", "row": 7, "script": "login",
            "diff": "@@ -14 +14 @@\n-a\n+b", "explanation": "The button is now called Log in.",
            "confidence": 0.82, "attempt": 1, "max_attempts": 3,
            "checks": ["applies", "transpiles"], "mode": "ask",
        });
        assert_eq!(
            describe_proposal(&data),
            "Row 8 of login: The button is now called Log in.\n\
             confidence 82% · attempt 1/3 · checks: applies, transpiles"
        );
        assert_eq!(proposal(&data), Some(("hp3", "@@ -14 +14 @@\n-a\n+b")));
        let ids: Vec<_> = proposal_actions().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, [SHOW, ACCEPT, REJECT, OPEN]);

        let mut auto = data.clone();
        auto["note"] = json!("confidence 0.5 is below 0.8");
        assert!(
            describe_proposal(&auto)
                .ends_with("Not applied automatically: confidence 0.5 is below 0.8")
        );
    }

    #[test]
    fn proposal_buttons_map_to_heal_commands() {
        assert_eq!(
            command_for(ACCEPT, "r1", None, Some("hp3")),
            Some(("run.heal_accept".into(), json!({"proposal_id": "hp3"})))
        );
        assert_eq!(
            command_for(REJECT, "r1", None, Some("hp3")),
            Some(("run.heal_reject".into(), json!({"proposal_id": "hp3"})))
        );
        // A proposal question never applies its diff as code; without a proposal there is
        // no accept.
        assert_eq!(command_for(APPLY, "r1", Some("diff"), Some("hp3")), None);
        assert_eq!(command_for(ACCEPT, "r1", None, None), None);
    }
}
