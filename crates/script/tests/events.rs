//! `browser.on(...)`: events pushed by a sink reach the script's handlers.
//!
//! A scripted sink plays a fixed list of events and records every command the script sends.
//! The same harness runs the example plugin `scripts/examples/plugins/hello-plugin` (the way
//! `crates/plugins` runs it, minus the real browser).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use bmcp_script::{CallFuture, CommandSink, EventFuture, RunOptions, RunOutcome, run_script};
use deno_core::serde_json::{Value, json};

type Calls = Rc<RefCell<Vec<(String, Value)>>>;

/// Plays `events` one by one (10 ms apart, so the script's own calls go first), then reports
/// "no more events". Answers `tabs.list` with two tabs and everything else with `{}`.
struct ScriptedSink {
    events: RefCell<VecDeque<(String, Value)>>,
    calls: Calls,
}

impl ScriptedSink {
    fn new(events: Vec<(&str, Value)>) -> (Rc<Self>, Calls) {
        let calls = Calls::default();
        let sink = ScriptedSink {
            events: RefCell::new(
                events
                    .into_iter()
                    .map(|(t, d)| (t.to_string(), d))
                    .collect(),
            ),
            calls: calls.clone(),
        };
        (Rc::new(sink), calls)
    }
}

impl CommandSink for ScriptedSink {
    fn call(&self, name: String, args: Value) -> CallFuture {
        self.calls.borrow_mut().push((name.clone(), args));
        Box::pin(async move {
            Ok(match name.as_str() {
                "tabs.list" => json!({ "tabs": [{ "id": 1 }, { "id": 2 }] }),
                _ => json!({}),
            })
        })
    }

    fn next_event(&self) -> EventFuture {
        let next = self.events.borrow_mut().pop_front();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            next
        })
    }
}

fn run(script: PathBuf, inputs: &[(&str, &str)], sink: Rc<ScriptedSink>) -> RunOutcome {
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    tokio.block_on(async {
        let opts = RunOptions {
            script,
            inputs: inputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            timeout: Some(Duration::from_secs(20)),
        };
        run_script(opts, sink).await.expect("run").0
    })
}

fn names(calls: &Calls) -> Vec<String> {
    calls.borrow().iter().map(|(n, _)| n.clone()).collect()
}

#[test]
fn handlers_get_matching_events_and_can_unsubscribe() {
    let (sink, calls) = ScriptedSink::new(vec![
        ("tab.created", json!({ "id": 1 })),
        ("other.thing", json!({ "id": 9 })),
        ("tab.updated", json!({ "id": 2 })),
        ("tab.closed", json!({ "id": 3 })),
    ]);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events.ts");
    let outcome = run(script, &[], sink);
    // The throwing handler is reported on stderr, not fatal; the run ends when events do.
    assert!(matches!(outcome, RunOutcome::Completed), "{outcome:?}");

    let calls = calls.borrow();
    let seen: Vec<&Value> = calls
        .iter()
        .filter(|(n, _)| n == "test.seen")
        .map(|(_, a)| a)
        .collect();
    // Only `tab.*` events, and only until the handler removed itself (after two).
    assert_eq!(
        seen,
        [
            &json!({ "topic": "tab.created", "id": 1 }),
            &json!({ "topic": "tab.updated", "id": 2 })
        ]
    );
    assert!(calls.contains(&("events.subscribe".into(), json!({ "topics": ["tab.*"] }))));
    assert!(calls.contains(&("events.unsubscribe".into(), json!({ "topics": ["tab.*"] }))));
    assert!(!calls.iter().any(|(n, _)| n == "test.never"));
}

#[test]
fn a_script_without_handlers_never_waits_for_events() {
    // output.ts never uses `browser.on`: it must end on its own, without subscribing.
    let (sink, calls) = ScriptedSink::new(vec![("tab.updated", json!({ "id": 1 }))]);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/output.ts");
    let outcome = run(script, &[], sink);
    assert!(matches!(outcome, RunOutcome::Completed), "{outcome:?}");
    assert!(!names(&calls).iter().any(|n| n.starts_with("events.")));
}

#[test]
fn hello_plugin_reacts_to_its_own_palette_command() {
    let (sink, calls) = ScriptedSink::new(vec![
        (
            "tab.updated",
            json!({ "id": 1, "url": "https://example.com/", "title": "Example", "loading": false }),
        ),
        (
            "plugin.command",
            json!({ "plugin": "someone-else", "id": "count-tabs" }),
        ),
        (
            "plugin.command",
            json!({ "plugin": "hello-plugin", "id": "count-tabs" }),
        ),
    ]);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/examples/plugins/hello-plugin/main.ts");
    let outcome = run(script, &[("plugin", "hello-plugin")], sink);
    assert!(matches!(outcome, RunOutcome::Completed), "{outcome:?}");

    let calls = calls.borrow();
    assert!(calls.contains(&(
        "events.subscribe".into(),
        json!({ "topics": ["tab.updated"] })
    )));
    assert!(calls.contains(&(
        "events.subscribe".into(),
        json!({ "topics": ["plugin.command"] })
    )));
    // One tabs.list: for its own command, not for the other plugin's.
    assert_eq!(calls.iter().filter(|(n, _)| n == "tabs.list").count(), 1);
}
