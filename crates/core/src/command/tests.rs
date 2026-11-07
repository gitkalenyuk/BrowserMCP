//! Tests of the registry pipeline (lookup, validation, locks, events, schemas, errors).

use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::TryRecvError;

use super::*;
use crate::backend::{BackendError, TabId};
use crate::commands::register_builtin;
use crate::events::{Event, EventBus, topics};
use crate::testing::{FakeBackend, ctx};

fn registry() -> Registry {
    let mut registry = Registry::new();
    register_builtin(&mut registry);
    registry
}

fn mcp() -> Caller {
    Caller::Mcp {
        session: "s1".into(),
        agent: Some("claude".into()),
    }
}

/// Collect every event received so far.
fn drain(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(event) => events.push(event),
            Err(TryRecvError::Empty) => return events,
            Err(other) => panic!("event bus: {other:?}"),
        }
    }
}

/// A hidden test command.
struct Secret;

#[derive(Serialize, JsonSchema)]
struct SecretOutput {
    value: String,
}

impl Command for Secret {
    const NAME: &'static str = "test.secret";
    type Args = Empty;
    type Output = SecretOutput;

    fn meta() -> Meta {
        Meta::new("Test only.", Toolset::Hidden).secret()
    }

    async fn run(_ctx: Ctx, _args: Empty) -> Result<SecretOutput, CmdError> {
        Ok(SecretOutput {
            value: "hunter2".into(),
        })
    }
}

#[test]
fn wire_name_replaces_dots() {
    assert_eq!(wire_name("tabs.list"), "tabs_list");
    assert_eq!(wire_name("net.rule.add"), "net_rule_add");
}

#[test]
fn list_is_sorted_and_filters_by_toolset() {
    let mut registry = registry();
    registry.add::<Secret>();

    let names: Vec<_> = registry.list(None).iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        [
            "agents.forget",
            "agents.list",
            "ai.handoff",
            "nav.goto",
            "nav.stop",
            "page.text",
            "page.zoom",
            "system.ping",
            "system.quit",
            "tabs.activate",
            "tabs.close",
            "tabs.closed",
            "tabs.closed_clear",
            "tabs.closed_reopen",
            "tabs.groups",
            "tabs.list",
            "tabs.lock",
            "tabs.open",
            "tabs.restore_closed",
            "tabs.split",
            "tabs.unlock",
            "tabs.unsplit"
        ]
    );

    let core = registry.list(Some(&[Toolset::Core]));
    assert_eq!(core.len(), 16);
    assert!(registry.list(Some(&[Toolset::Net])).is_empty());

    // Hidden: the test's secret command and the UI plumbing `ui.layout`.
    let hidden = registry.list(Some(&[Toolset::Hidden]));
    assert_eq!(hidden.len(), 2);
    assert!(hidden.iter().any(|c| c.sensitivity == Sensitivity::Secret));

    let info = registry.get("nav_goto").unwrap();
    assert_eq!(info.name, "nav.goto");
    assert_eq!(info.wire_name, "nav_goto");
    assert!(info.mutates_tab);
    assert_eq!(info.palette, Some("Go to URL…"));
    assert_eq!(registry.get("nav.goto"), Some(info));
    assert!(registry.get("nav").is_none());
}

#[test]
fn toolset_parses_from_str() {
    assert_eq!("net".parse::<Toolset>().unwrap(), Toolset::Net);
    assert_eq!(" core".parse::<Toolset>().unwrap(), Toolset::Core);
    assert_eq!(
        "nope".parse::<Toolset>().unwrap_err().code,
        ErrorCode::InvalidParams
    );
}

#[tokio::test]
async fn call_by_dotted_and_wire_name() {
    let registry = registry();
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);
    let dotted = registry
        .call(ctx(mcp(), fake.clone()), "tabs.list", json!({}))
        .await
        .unwrap();
    let wire = registry
        .call(ctx(mcp(), fake.clone()), "tabs_list", json!({}))
        .await
        .unwrap();
    assert_eq!(dotted, wire);
    assert_eq!(dotted["tabs"][0]["id"], 1);
}

#[tokio::test]
async fn invalid_params_name_the_field() {
    let registry = registry();
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);
    let call = |name: &'static str, args: Value| {
        registry.call(ctx(Caller::Rest, fake.clone()), name, args)
    };

    let err = call("nav.goto", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert_eq!(err.retry, Retry::No);
    assert!(
        err.message.contains("missing field `url`"),
        "{}",
        err.message
    );
    assert!(err.hint.as_deref().unwrap().contains("nav_goto"));

    let err = call("tabs.open", json!({ "url": 5 })).await.unwrap_err();
    assert!(err.message.contains("`url`"), "{}", err.message);
    assert!(err.message.contains("invalid type"), "{}", err.message);

    let err = call("page.text", json!({ "tab": "first" }))
        .await
        .unwrap_err();
    assert!(err.message.contains("`tab`"), "{}", err.message);

    let err = call("nav.goto", json!({ "url": "x", "tab_id": 1 }))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("unknown field `tab_id`"),
        "{}",
        err.message
    );

    let err = call("tabs.list", json!([1, 2])).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);

    // Nothing reached the browser.
    assert!(fake.calls().is_empty(), "{:?}", fake.calls());
}

#[tokio::test]
async fn unknown_command_is_not_found_with_candidates() {
    let registry = registry();
    let err = registry
        .call(ctx(mcp(), FakeBackend::new()), "tabs_lst", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.retry, Retry::No);
    assert_eq!(
        err.candidates,
        vec![
            json!("tabs_activate"),
            json!("tabs_close"),
            json!("tabs_closed"),
            json!("tabs_closed_clear"),
            json!("tabs_closed_reopen"),
            json!("tabs_groups"),
            json!("tabs_list"),
            json!("tabs_lock"),
            json!("tabs_open"),
            json!("tabs_restore_closed"),
            json!("tabs_split"),
            json!("tabs_unlock"),
            json!("tabs_unsplit")
        ]
    );

    let err = registry
        .call(ctx(mcp(), FakeBackend::new()), "tabs.lst", json!({}))
        .await
        .unwrap_err();
    assert_eq!(
        err.candidates,
        vec![
            json!("tabs.activate"),
            json!("tabs.close"),
            json!("tabs.closed"),
            json!("tabs.closed_clear"),
            json!("tabs.closed_reopen"),
            json!("tabs.groups"),
            json!("tabs.list"),
            json!("tabs.lock"),
            json!("tabs.open"),
            json!("tabs.restore_closed"),
            json!("tabs.split"),
            json!("tabs.unlock"),
            json!("tabs.unsplit")
        ]
    );
}

/// Tab 1 belongs to agent "owner"; everyone else is refused.
struct OwnedTab1;

impl LockPolicy for OwnedTab1 {
    fn check(&self, caller: &Caller, tab: Option<TabId>) -> Result<(), CmdError> {
        let is_owner = matches!(caller, Caller::Agent { id } if id == "owner");
        if tab == Some(TabId(1)) && !is_owner {
            return Err(CmdError::tab_locked(TabId(1), "agent owner"));
        }
        Ok(())
    }
}

#[tokio::test]
async fn lock_policy_rejects_with_tab_locked() {
    let mut registry = registry();
    registry.set_lock_policy(Arc::new(OwnedTab1));
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);

    // Explicit tab: refused by the registry before running.
    let err = registry
        .call(
            ctx(mcp(), fake.clone()),
            "nav.goto",
            json!({ "url": "https://evil.test/", "tab": 1 }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TabLocked);
    assert_eq!(err.retry, Retry::AfterWait);

    // Default tab (resolves to 1): refused by `Ctx::resolve_tab`.
    let err = registry
        .call(
            ctx(mcp(), fake.clone()),
            "nav.goto",
            json!({ "url": "https://evil.test/" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TabLocked);
    assert!(
        !fake.calls().iter().any(|c| c.starts_with("navigate")),
        "{:?}",
        fake.calls()
    );

    // Reading is not lock-checked; the owner may navigate.
    registry
        .call(ctx(mcp(), fake.clone()), "page.text", json!({ "tab": 1 }))
        .await
        .unwrap();
    let owner = Caller::Agent { id: "owner".into() };
    registry
        .call(
            ctx(owner, fake.clone()),
            "nav.goto",
            json!({ "url": "https://ok.test/", "tab": 1 }),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn events_are_emitted_in_seq_order() {
    let registry = registry();
    let fake = FakeBackend::with_tabs(&["https://a.test/"]);
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let context = Ctx::new(mcp(), fake.clone(), bus.clone());

    registry
        .call(context.clone(), "tabs_list", json!({}))
        .await
        .unwrap();
    registry
        .call(context.clone(), "nav.goto", json!({ "url": "x", "tab": 9 }))
        .await
        .unwrap_err();
    registry
        .call(context.clone(), "tabs.open", json!({ "url": 5 }))
        .await
        .unwrap_err();

    // The agent's `agents.updated` / `tab.updated` events are interleaved; only look at
    // the command events here.
    let events: Vec<Event> = drain(&mut rx)
        .into_iter()
        .filter(|e| e.topic.starts_with("command."))
        .collect();
    let topics_seen: Vec<_> = events.iter().map(|e| e.topic.as_str()).collect();
    assert_eq!(
        topics_seen,
        [
            topics::COMMAND_STARTED,
            topics::COMMAND_FINISHED,
            topics::COMMAND_STARTED,
            topics::COMMAND_FINISHED,
            topics::COMMAND_REJECTED,
        ]
    );
    let seqs: Vec<_> = events.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");

    let started = &events[0].data;
    assert_eq!(started["name"], "tabs.list");
    assert_eq!(started["call_id"], 1);
    assert_eq!(
        started["caller"],
        json!({ "kind": "mcp", "session": "s1", "agent": "claude" })
    );
    assert_eq!(started["args"], json!({}));

    let finished = &events[1].data;
    assert_eq!(finished["call_id"], 1);
    assert_eq!(finished["ok"], true);
    assert_eq!(finished["error"], Value::Null);
    assert!(finished["duration_ms"].is_u64());

    assert_eq!(events[2].data["tab"], 9);
    assert_eq!(events[3].data["ok"], false);
    assert_eq!(events[3].data["error"], "TAB_NOT_FOUND");

    let rejected = &events[4].data;
    assert_eq!(rejected["name"], "tabs.open");
    assert_eq!(rejected["error"], "INVALID_PARAMS");
    assert_eq!(rejected["call_id"], 3);
}

#[test]
fn schemas_are_inlined_objects() {
    let registry = registry();
    for info in registry.list(None) {
        for (what, schema) in [("args", &info.args_schema), ("output", &info.output_schema)] {
            assert_eq!(schema["type"], "object", "{} {what}: {schema}", info.name);
            let text = schema.to_string();
            assert!(
                !text.contains("$ref"),
                "{} {what} has $ref: {text}",
                info.name
            );
            assert!(
                !text.contains("$defs"),
                "{} {what} has $defs: {text}",
                info.name
            );
            assert!(schema.get("$schema").is_none(), "{} {what}", info.name);
        }
    }
    // Nested types are inlined, with doc comments as descriptions.
    let tabs = &registry.get("tabs.list").unwrap().output_schema;
    let item = &tabs["properties"]["tabs"]["items"];
    assert_eq!(item["type"], "object");
    assert_eq!(item["properties"]["url"]["description"], "Current URL.");
    // Unknown fields are refused, and the schema says so.
    let goto = &registry.get("nav.goto").unwrap().args_schema;
    assert_eq!(goto["additionalProperties"], false);
    assert_eq!(goto["required"], json!(["url"]));
}

#[test]
#[should_panic(expected = "registered twice")]
fn duplicate_command_panics() {
    let mut registry = registry();
    registry.add::<crate::commands::tabs::List>();
}

struct BadName;

impl Command for BadName {
    const NAME: &'static str = "Tabs.List";
    type Args = Empty;
    type Output = Empty;
    fn meta() -> Meta {
        Meta::new("bad", Toolset::Hidden)
    }
    async fn run(_ctx: Ctx, _args: Empty) -> Result<Empty, CmdError> {
        Ok(Empty {})
    }
}

#[test]
#[should_panic(expected = "invalid command name")]
fn bad_name_panics() {
    Registry::new().add::<BadName>();
}

struct ListOutput;

impl Command for ListOutput {
    const NAME: &'static str = "test.vec";
    type Args = Empty;
    type Output = Vec<u32>;
    fn meta() -> Meta {
        Meta::new("bad", Toolset::Hidden)
    }
    async fn run(_ctx: Ctx, _args: Empty) -> Result<Vec<u32>, CmdError> {
        Ok(vec![])
    }
}

#[test]
#[should_panic(expected = "Output must be a struct")]
fn non_object_output_panics() {
    Registry::new().add::<ListOutput>();
}

#[test]
fn backend_errors_map_to_cmd_errors() {
    let cases = [
        (
            BackendError::TabNotFound(TabId(4)),
            ErrorCode::TabNotFound,
            Retry::No,
        ),
        (BackendError::NoTabs, ErrorCode::NoTabs, Retry::No),
        (
            BackendError::Timeout(Duration::from_secs(5)),
            ErrorCode::Timeout,
            Retry::AfterWait,
        ),
        (
            BackendError::ShuttingDown,
            ErrorCode::ShuttingDown,
            Retry::No,
        ),
        (
            BackendError::Cdp("boom".into()),
            ErrorCode::Internal,
            Retry::No,
        ),
    ];
    for (backend, code, retry) in cases {
        let message = backend.to_string();
        let err = CmdError::from(backend);
        assert_eq!((err.code, err.retry), (code, retry), "{message}");
        assert_eq!(err.message, message);
    }
}

#[test]
fn cmd_error_wire_format() {
    let err = CmdError::from(BackendError::TabNotFound(TabId(7)));
    assert_eq!(
        serde_json::to_value(&err).unwrap(),
        json!({
            "code": "TAB_NOT_FOUND",
            "message": "tab 7 not found",
            "retry": "no",
            "hint": "Call tabs_list to see the open tabs.",
        })
    );
    assert_eq!(err.to_string(), "TAB_NOT_FOUND: tab 7 not found");

    let err = CmdError::new(ErrorCode::AmbiguousTarget, "2 buttons match")
        .with_candidates(vec![json!("@3:e1"), json!("@3:e2")]);
    let value = serde_json::to_value(&err).unwrap();
    assert_eq!(value["retry"], "after_snapshot");
    assert_eq!(value["candidates"], json!(["@3:e1", "@3:e2"]));
    let back: CmdError = serde_json::from_value(value).unwrap();
    assert_eq!(back, err);

    // Every code serializes to its SCREAMING_SNAKE_CASE `as_str`.
    for code in [
        ErrorCode::StaleRef,
        ErrorCode::NavigationInterrupted,
        ErrorCode::BlockedInStealth,
        ErrorCode::NeedsApproval,
        ErrorCode::DialogOpen,
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
    }
}

/// A command that takes 300 ms.
struct Slow;

impl Command for Slow {
    const NAME: &'static str = "test.slow";
    type Args = Empty;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new("Test only.", Toolset::Hidden)
    }

    async fn run(_ctx: Ctx, _args: Empty) -> Result<Empty, CmdError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(Empty {})
    }
}

/// Fires for `test.slow` only, after `after` (zero = at once).
struct TestGuard {
    after: Duration,
}

impl CommandGuard for TestGuard {
    fn watch(
        &self,
        _ctx: &Ctx,
        name: &'static str,
        _args: &Value,
    ) -> Option<crate::BoxFuture<'static, CmdError>> {
        if name != "test.slow" {
            return None;
        }
        let after = self.after;
        Some(Box::pin(async move {
            tokio::time::sleep(after).await;
            CmdError::new(ErrorCode::DialogOpen, "alert \"hi\" is open")
        }))
    }
}

#[tokio::test]
async fn guards_stop_commands_before_and_while_they_run() {
    let fake = FakeBackend::with_tabs(&["https://a.test"]);
    for after in [Duration::ZERO, Duration::from_millis(30)] {
        let mut registry = registry();
        registry.add::<Slow>();
        registry.add_guard(Arc::new(TestGuard { after }));
        let started = std::time::Instant::now();
        let err = registry
            .call(ctx(mcp(), fake.clone()), "test.slow", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::DialogOpen);
        assert_eq!(err.retry, Retry::No);
        // Cancelled when the guard fired, not after the command's 300 ms.
        assert!(started.elapsed() < Duration::from_millis(250));
        // Commands the guard does not watch run normally.
        registry
            .call(ctx(mcp(), fake.clone()), "tabs.list", json!({}))
            .await
            .unwrap();
    }
    // Without guards the slow command finishes.
    let mut registry = registry();
    registry.add::<Slow>();
    registry
        .call(ctx(mcp(), fake), "test.slow", json!({}))
        .await
        .unwrap();
}
