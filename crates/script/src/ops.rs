//! The Rust functions ("ops") that JavaScript can call, bundled with the JS
//! prelude into one deno_core `Extension` named `bmcp`.
//!
//! Only a handful of ops exist on purpose. Every browser command goes through the one
//! generic `op_bmcp_call`, so adding a browser command never touches this file.
//! `op_bmcp_next_event` feeds `browser.on(topic, handler)` (plugins, long-lived scripts).

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::Duration;

// op2 needs the spelled-out `serde_json::Value` path in op signatures.
use deno_core::serde_json::{self, Value};
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

use crate::sink::CommandSink;

/// Stored in `OpState` (deno_core's per-isolate "bag of state", keyed by type)
/// so ops can find the sink.
#[derive(Clone)]
pub struct SinkHandle(pub Rc<dyn CommandSink>);

/// The `inputs` object (from `--input key=value`), also kept in `OpState`.
pub struct Inputs(pub Value);

/// `browser.<ns>.<method>(args)` ends up here as `name = "<ns>.<method>"`.
/// `async` op: JS gets a Promise; the isolate keeps running other work.
#[op2]
#[serde]
async fn op_bmcp_call(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] args: serde_json::Value,
) -> Result<serde_json::Value, JsErrorBox> {
    // Clone the Rc and release the RefCell borrow BEFORE awaiting:
    // holding a borrow across `.await` would panic if another op ran meanwhile.
    let sink = state.borrow().borrow::<SinkHandle>().0.clone();
    let args = with_tab_defaults(&name, args, tab_defaults());
    sink.call(name.clone(), args)
        .await
        .map_err(|e| JsErrorBox::generic(format!("browser.{name} failed: {e}")))
}

/// `BMCP_TAB_DEFAULTS` (set by mass runs, e.g. `{"profile": "run-1-r3-a1", "incognito":
/// true, "window": "offscreen", "run": "run-1"}`), read once.
fn tab_defaults() -> &'static serde_json::Value {
    static DEFAULTS: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(|| {
        std::env::var("BMCP_TAB_DEFAULTS")
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(serde_json::Value::Null)
    })
}

/// For `tabs.open`: add every default the script did not set itself (so each row of a
/// mass run opens its tabs in its own profile/window without changing the script).
pub fn with_tab_defaults(
    name: &str,
    mut args: serde_json::Value,
    defaults: &serde_json::Value,
) -> serde_json::Value {
    let (Some(defaults), true) = (defaults.as_object(), name == "tabs.open") else {
        return args;
    };
    if args.is_null() {
        args = serde_json::json!({});
    }
    if let Some(object) = args.as_object_mut() {
        for (key, value) in defaults {
            object.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    args
}

/// Next browser event as `{topic, data}`, or `null` when no more events will come (the
/// prelude's dispatch loop then stops, so the script can finish).
#[op2]
#[serde]
async fn op_bmcp_next_event(state: Rc<RefCell<OpState>>) -> serde_json::Value {
    let sink = state.borrow().borrow::<SinkHandle>().0.clone();
    match sink.next_event().await {
        Some((topic, data)) => serde_json::json!({ "topic": topic, "data": data }),
        None => Value::Null,
    }
}

/// Returns the inputs object; the prelude freezes it into `globalThis.inputs`.
#[op2]
#[serde]
fn op_bmcp_inputs(state: &mut OpState) -> serde_json::Value {
    state.borrow::<Inputs>().0.clone()
}

/// `console.log` & co. Writes one line to stdout (or stderr).
#[op2(fast)]
fn op_bmcp_log(#[string] line: &str, to_stderr: bool) {
    // Errors writing to a closed pipe are ignored: logging must never crash a script.
    if to_stderr {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    } else {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// `await sleep(ms)`.
#[op2]
async fn op_bmcp_sleep(ms: f64) {
    let ms = if ms.is_finite() && ms > 0.0 { ms } else { 0.0 };
    tokio::time::sleep(Duration::from_secs_f64(ms / 1000.0)).await;
}

// Generates `bmcp::init(sink, inputs) -> Extension`. The `state` closure runs
// while the isolate is created, BEFORE prelude.js executes, so the prelude can
// already read `inputs` (putting state in afterwards would be too late).
deno_core::extension!(
    bmcp,
    ops = [op_bmcp_call, op_bmcp_inputs, op_bmcp_log, op_bmcp_sleep, op_bmcp_next_event],
    esm_entry_point = "ext:bmcp/prelude.js",
    esm = [dir "src/js", "prelude.js"],
    options = { sink: SinkHandle, inputs: Inputs },
    state = |state, options| {
        state.put(options.sink);
        state.put(options.inputs);
    },
);

#[cfg(test)]
mod tests {
    use deno_core::serde_json::{self, json};

    use super::with_tab_defaults;

    #[test]
    fn tab_defaults_fill_only_missing_tabs_open_args() {
        let defaults = json!({"profile": "p1", "window": "offscreen", "run": "r"});
        assert_eq!(
            with_tab_defaults("tabs.open", json!({"url": "u", "window": "new"}), &defaults),
            json!({"url": "u", "window": "new", "profile": "p1", "run": "r"})
        );
        assert_eq!(
            with_tab_defaults("tabs.open", serde_json::Value::Null, &defaults),
            defaults
        );
        // Other commands and missing defaults: untouched.
        assert_eq!(
            with_tab_defaults("nav.goto", json!({"url": "u"}), &defaults),
            json!({"url": "u"})
        );
        assert_eq!(
            with_tab_defaults("tabs.open", json!({"url": "u"}), &serde_json::Value::Null),
            json!({"url": "u"})
        );
    }
}
