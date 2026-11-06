//! API-driven actions: `command.started` / `command.finished` events → [`Step`]s, and refs →
//! semantic locators.
//!
//! The registry emits `command.started {call_id, name, caller, args, tab}` before every call
//! and `command.finished {call_id, ok, error, duration_ms}` after it (see
//! `crates/core/src/command/registry.rs`). The finished event carries no output, so what we log
//! comes from the arguments; a ref (`@4:e3`) is turned into role + name with the perception
//! state the call itself used (see [`resolve_target`]).

use bmcp_browser::PerceptionState;
use bmcp_core::{Ctx, TabId};
use bmcp_perception::parse_ref;
use serde_json::{Value, json};

use crate::model::{CommandCall, Source, Step, Target};

/// Command groups that are recorded.
const LOGGED_GROUPS: [&str; 4] = ["page.", "nav.", "input.", "tabs."];

/// Recorded commands that never change the page: no dedupe window, codegen skips them
/// (`nav.wait` is the exception there: it becomes a wait in the script).
const READ_ONLY: [&str; 9] = [
    "page.snapshot",
    "page.find",
    "page.read",
    "page.diff",
    "page.screenshot",
    "page.text",
    "page.ax_tree",
    "tabs.list",
    "nav.wait",
];

/// Is this command recorded at all?
pub fn is_logged(name: &str) -> bool {
    LOGGED_GROUPS.iter().any(|g| name.starts_with(g))
}

/// Does it act on the page (so trusted events while it runs are its own, not a person's)?
pub fn acts_on_page(name: &str) -> bool {
    is_logged(name) && !READ_ONLY.contains(&name)
}

pub fn is_read_only(name: &str) -> bool {
    READ_ONLY.contains(&name)
}

/// The steps for one `command.started` event (one per batch step for `page.act_batch`).
/// They are `pending` until `command.finished` arrives.
pub fn steps_for_call(data: &Value, ts_ms: u64, page_url: &str) -> Vec<Step> {
    let name = data["name"].as_str().unwrap_or_default().to_string();
    let args = data["args"].clone();
    let call = CommandCall {
        name: name.clone(),
        call_id: data["call_id"].as_u64().unwrap_or(0),
        caller: data["caller"].clone(),
        args: args.clone(),
        ..CommandCall::default()
    };
    let base = Step {
        ts_ms,
        source: Source::Api,
        url: Some(page_url.to_string()).filter(|u| !u.is_empty()),
        command: Some(call),
        pending: true,
        ..Step::default()
    };
    let text = |v: &Value| v.as_str().map(str::to_string);
    let element = |step: &Value, index: Option<usize>| {
        let mut s = base.clone();
        s.action = text(&step["action"]).unwrap_or_else(|| "command".into());
        s.value = text(&step["value"]);
        s.target = text(&step["ref"]).map(|r| Target {
            ref_: Some(r),
            ..Target::default()
        });
        if let Some(i) = index {
            // A batch step keeps only its own arguments, so redacting one step's secret
            // cannot leave the value behind in a sibling's copy of the whole batch.
            s.extra = Some(json!({ "batch_index": i }));
            if let Some(call) = &mut s.command {
                call.args = step.clone();
            }
        }
        s
    };
    match name.as_str() {
        "page.act" => vec![element(&args, None)],
        "page.act_batch" => args["steps"]
            .as_array()
            .map(|steps| {
                steps
                    .iter()
                    .enumerate()
                    .map(|(i, s)| element(s, Some(i)))
                    .collect()
            })
            .unwrap_or_default(),
        "input.keys" => vec![Step {
            action: "press".into(),
            value: text(&args["keys"]),
            ..base
        }],
        "nav.goto" | "tabs.open" => vec![Step {
            action: if name == "nav.goto" {
                "goto"
            } else {
                "open_tab"
            }
            .into(),
            url: text(&args["url"]),
            ..base
        }],
        "nav.back" | "nav.forward" | "nav.reload" => vec![Step {
            action: name.trim_start_matches("nav.").into(),
            ..base
        }],
        "nav.wait" => vec![Step {
            action: "wait".into(),
            wait_for: text(&args["for"]),
            value: text(&args["value"]),
            target: text(&args["value"])
                .filter(|_| args["for"] == "ref")
                .map(|r| Target {
                    ref_: Some(r),
                    ..Target::default()
                }),
            ..base
        }],
        _ => vec![Step {
            action: "command".into(),
            ..base
        }],
    }
}

/// Words that mark a field as secret when the snapshot did not say `pwd`.
fn looks_secret(name: &str) -> bool {
    let name = name.to_lowercase();
    [
        "password",
        "passwort",
        "пароль",
        "passcode",
        "pin code",
        "cvv",
        "cvc",
    ]
    .iter()
    .any(|w| name.contains(w))
}

/// Mark the step secret if its target is a password field, and if so replace the value
/// everywhere it was stored (step value and command args) with `"***"`.
pub fn redact_if_secret(step: &mut Step, pwd_state: bool) {
    let typed = matches!(step.action.as_str(), "fill" | "type");
    let by_name = step
        .target
        .as_ref()
        .and_then(|t| t.name.as_deref())
        .is_some_and(looks_secret);
    let by_type = step.target.as_ref().and_then(|t| t.input_type.as_deref()) == Some("password");
    if typed && (pwd_state || by_name || by_type) {
        step.secret = true;
    }
    if !step.secret {
        return;
    }
    step.value = Some("***".into());
    if let Some(Value::Object(map)) = step.command.as_mut().map(|c| &mut c.args)
        && map.contains_key("value")
    {
        map.insert("value".into(), json!("***"));
    }
}

/// Role, name and `nth` of the element behind `reference` (`@4:e3`), plus whether the
/// snapshot marked it `pwd`.
///
/// 1. The newest remembered snapshot that printed this e-number (e-numbers are never reused
///    in a tab, so this works even after the page navigated away). `page.act` itself takes a
///    "before" snapshot, so the element is nearly always there.
/// 2. Else, if the ref is still valid: its DOM node's accessibility node
///    (`Accessibility.getPartialAXTree`).
///
/// Called after `command.finished`, when the command no longer holds the tab's state lock.
pub async fn resolve_target(ctx: &Ctx, tab: TabId, reference: &str) -> Option<(Target, bool)> {
    let number = parse_ref(reference).ok()?.number;
    let state = PerceptionState::from_ctx(ctx).tab(tab);
    let state = state.lock().await;
    for (snap, _) in state.history.iter().rev() {
        let same_number = |r: &Option<String>| {
            r.as_deref()
                .and_then(|r| parse_ref(r).ok())
                .is_some_and(|p| p.number == number)
        };
        let Some(pos) = snap.nodes.iter().position(|n| same_number(&n.ref_str)) else {
            continue;
        };
        let node = &snap.nodes[pos];
        let nth = snap.nodes[..pos]
            .iter()
            .filter(|n| n.ref_str.is_some() && n.role == node.role && n.name == node.name)
            .count();
        let target = Target {
            ref_: Some(reference.to_string()),
            role: Some(node.role.clone()),
            name: Some(node.name.clone()).filter(|n| !n.is_empty()),
            nth: Some(nth).filter(|n| *n > 0),
            from: Some("snapshot".into()),
            ..Target::default()
        };
        return Some((target, node.states.iter().any(|s| s == "pwd")));
    }
    let backend_node = state.registry.resolve(reference).ok()?;
    drop(state);
    let reply = ctx
        .backend
        .cdp(
            tab,
            "Accessibility.getPartialAXTree".into(),
            json!({ "backendNodeId": backend_node, "fetchRelatives": false }),
        )
        .await
        .ok()?;
    let nodes = reply["nodes"].as_array()?;
    let node = nodes
        .iter()
        .find(|n| n["backendDOMNodeId"].as_i64() == Some(backend_node))
        .or_else(|| nodes.first())?;
    let text = |v: &Value| v.as_str().map(str::to_string).filter(|s| !s.is_empty());
    Some((
        Target {
            ref_: Some(reference.to_string()),
            role: text(&node["role"]["value"]),
            name: text(&node["name"]["value"]),
            from: Some("ax".into()),
            ..Target::default()
        },
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn act_and_batch_become_steps() {
        let data = json!({
            "call_id": 7, "name": "page.act", "caller": {"kind": "rest"},
            "args": {"ref": "@3:e2", "action": "fill", "value": "ann@example.test"}, "tab": null
        });
        let steps = steps_for_call(&data, 100, "http://x/login");
        assert_eq!(steps.len(), 1);
        let s = &steps[0];
        assert_eq!(
            (s.action.as_str(), s.value.as_deref()),
            ("fill", Some("ann@example.test"))
        );
        assert_eq!(s.target.as_ref().unwrap().ref_.as_deref(), Some("@3:e2"));
        assert!(s.pending);

        let batch = json!({
            "call_id": 8, "name": "page.act_batch", "caller": {"kind": "rest"},
            "args": {"steps": [{"ref": "e2", "action": "fill", "value": "a"}, {"ref": "e5", "action": "click"}]}
        });
        let steps = steps_for_call(&batch, 100, "");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].action, "click");
        assert_eq!(steps[1].extra, Some(json!({"batch_index": 1})));
    }

    #[test]
    fn nav_and_other_commands() {
        let goto = json!({"call_id": 1, "name": "nav.goto", "args": {"url": "http://x/"}});
        assert_eq!(steps_for_call(&goto, 1, "")[0].action, "goto");
        assert_eq!(
            steps_for_call(&goto, 1, "")[0].url.as_deref(),
            Some("http://x/")
        );
        let wait =
            json!({"call_id": 2, "name": "nav.wait", "args": {"for": "text", "value": "Welcome"}});
        let w = &steps_for_call(&wait, 1, "")[0];
        assert_eq!(
            (w.action.as_str(), w.wait_for.as_deref(), w.value.as_deref()),
            ("wait", Some("text"), Some("Welcome"))
        );
        let snap = json!({"call_id": 3, "name": "page.snapshot", "args": {}});
        assert_eq!(steps_for_call(&snap, 1, "")[0].action, "command");
        assert!(is_logged("page.snapshot") && !acts_on_page("page.snapshot"));
        assert!(acts_on_page("page.act") && !is_logged("record.start"));
    }

    #[test]
    fn secrets_are_redacted_in_value_and_args() {
        let data = json!({"call_id": 9, "name": "page.act",
            "args": {"ref": "@3:e4", "action": "fill", "value": "hunter2"}});
        let mut step = steps_for_call(&data, 1, "").remove(0);
        step.target.as_mut().unwrap().name = Some("Password".into());
        redact_if_secret(&mut step, false);
        assert!(step.secret);
        assert_eq!(step.value.as_deref(), Some("***"));
        assert_eq!(step.command.unwrap().args["value"], "***");

        // Batch: only the secret step's value is replaced.
        let batch = json!({"call_id": 10, "name": "page.act_batch", "args": {"steps": [
            {"ref": "e2", "action": "fill", "value": "ann"},
            {"ref": "e4", "action": "fill", "value": "hunter2"}]}});
        let mut steps = steps_for_call(&batch, 1, "");
        redact_if_secret(&mut steps[1], true);
        assert_eq!(steps[0].command.as_ref().unwrap().args["value"], "ann");
        assert_eq!(steps[1].command.as_ref().unwrap().args["value"], "***");
        // No step keeps a copy of the whole batch.
        assert!(
            !serde_json::to_string(&steps[0])
                .unwrap()
                .contains("hunter2")
        );

        // A click on a "Password" button is not a secret.
        let click =
            json!({"call_id": 11, "name": "page.act", "args": {"ref": "e9", "action": "click"}});
        let mut step = steps_for_call(&click, 1, "").remove(0);
        step.target.as_mut().unwrap().name = Some("Forgot password?".into());
        redact_if_secret(&mut step, false);
        assert!(!step.secret);
    }
}
