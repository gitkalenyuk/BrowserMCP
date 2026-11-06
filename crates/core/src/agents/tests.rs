//! Tests of agent identity, tab ownership, locks, takeover and handoff — through the
//! registry, like the API surfaces call them.

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::command::{Registry, Retry};
use crate::commands::register_builtin;
use crate::testing::FakeBackend;

struct World {
    registry: Arc<Registry>,
    fake: Arc<FakeBackend>,
    bus: EventBus,
}

fn world(urls: &[&str]) -> World {
    let mut registry = Registry::new();
    register_builtin(&mut registry);
    World {
        registry: Arc::new(registry),
        fake: FakeBackend::with_tabs(urls),
        bus: EventBus::new(),
    }
}

fn agent(id: &str) -> Caller {
    Caller::Mcp {
        session: format!("session-{id}"),
        agent: Some(id.into()),
    }
}

impl World {
    async fn call(&self, caller: Caller, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(caller, self.fake.clone(), self.bus.clone());
        self.registry.call(ctx, name, args).await
    }

    async fn ok(&self, caller: Caller, name: &str, args: Value) -> Value {
        self.call(caller, name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    /// The `tabs.list` entry of `tab`.
    async fn tab(&self, tab: i32) -> Value {
        let list = self.ok(Caller::Ui, "tabs.list", json!({})).await;
        list["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == tab)
            .cloned()
            .unwrap()
    }

    fn agents(&self) -> Arc<Agents> {
        self.registry.extensions().get::<Agents>().unwrap()
    }
}

fn goto(tab: i32) -> Value {
    json!({ "url": "https://x.test/", "tab": tab })
}

#[tokio::test]
async fn first_mutation_claims_the_tab_and_locks_out_other_agents() {
    let w = world(&["https://a.test/", "https://b.test/"]);
    w.ok(agent("alice"), "nav.goto", goto(1)).await;

    let tab = w.tab(1).await;
    assert_eq!(tab["groupId"], "agent:alice");
    assert_eq!(tab["lockedBy"], "alice");
    assert_eq!(tab["lock"]["state"], "active");
    assert_eq!(w.tab(2).await["lock"], Value::Null);

    // Another agent is refused with the owner and "retry after waiting".
    let err = w.call(agent("bob"), "nav.goto", goto(1)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::TabLocked);
    assert_eq!(err.owner.as_deref(), Some("alice"));
    assert_eq!(err.retry, Retry::AfterWait);
    assert_eq!(serde_json::to_value(&err).unwrap()["owner"], json!("alice"));
    // Reading someone else's tab is fine; the owner keeps working.
    w.ok(agent("bob"), "page.text", json!({ "tab": 1 })).await;
    w.ok(agent("alice"), "nav.goto", goto(1)).await;
    // REST / scripts respect agent locks too.
    let err = w.call(Caller::Rest, "nav.goto", goto(1)).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::TabLocked);
}

#[tokio::test]
async fn opened_tabs_belong_to_the_agent_and_are_its_default_tab() {
    let w = world(&["https://a.test/"]);
    let out = w
        .ok(
            agent("alice"),
            "tabs.open",
            json!({ "url": "https://n.test/" }),
        )
        .await;
    assert_eq!(out["groupId"], "agent:alice");
    assert_eq!(out["lockedBy"], "alice");
    // Bob opens a newer tab; Alice's default tab is still her own (not the highest id).
    w.ok(agent("bob"), "tabs.open", json!({})).await;
    let out = w
        .ok(
            agent("alice"),
            "nav.goto",
            json!({ "url": "https://y.test/" }),
        )
        .await;
    assert_eq!(out["id"], 2);
    // A human (no own tab) still gets the newest tab.
    let out = w.ok(Caller::Rest, "page.text", json!({})).await;
    assert_eq!(out["tab"], 3);

    let groups = w.ok(Caller::Ui, "tabs.groups", json!({})).await;
    assert_eq!(
        groups["groups"],
        json!([
            { "id": "agent:alice", "name": "alice", "color": "blue", "agentId": "alice", "tabs": [2] },
            { "id": "agent:bob", "name": "bob", "color": "orange", "agentId": "bob", "tabs": [3] },
        ])
    );
}

#[tokio::test]
async fn human_action_takes_over_and_lock_gives_back() {
    let w = world(&["https://a.test/"]);
    w.ok(agent("alice"), "nav.goto", goto(1)).await;

    // The user types a URL into the omnibox of Alice's tab: takeover.
    w.ok(Caller::Ui, "nav.goto", goto(1)).await;
    let tab = w.tab(1).await;
    assert_eq!(tab["lock"]["state"], "paused");
    assert_eq!(tab["lockedBy"], Value::Null);
    assert_eq!(tab["groupId"], "agent:alice"); // still in her group
    let err = w
        .call(agent("alice"), "nav.goto", goto(1))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TabLocked);
    assert_eq!(err.owner.as_deref(), Some(USER));
    // While the user has it, REST/scripts act for the user.
    w.ok(Caller::Rest, "nav.goto", goto(1)).await;

    // "Give back": the user locks it for Alice again.
    let out = w.ok(Caller::Ui, "tabs.lock", json!({ "tab": 1 })).await;
    assert_eq!(out["lock"]["state"], "active");
    assert_eq!(out["lock"]["owner"], "alice");
    w.ok(agent("alice"), "nav.goto", goto(1)).await;
}

#[tokio::test]
async fn unlock_is_take_over_for_humans_and_release_for_the_owner() {
    let w = world(&["https://a.test/"]);
    w.ok(agent("alice"), "nav.goto", goto(1)).await;

    let err = w
        .call(agent("bob"), "tabs.unlock", json!({ "tab": 1 }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TabLocked);

    // "Take over" button.
    let out = w.ok(Caller::Ui, "tabs.unlock", json!({ "tab": 1 })).await;
    assert_eq!(out["lock"]["state"], "paused");
    w.ok(Caller::Ui, "tabs.lock", json!({ "tab": 1 })).await;

    // The owner releases: the tab is free, Bob can take it.
    let out = w
        .ok(agent("alice"), "tabs.unlock", json!({ "tab": 1 }))
        .await;
    assert_eq!(out["lock"], Value::Null);
    w.ok(agent("bob"), "nav.goto", goto(1)).await;
    assert_eq!(w.tab(1).await["lockedBy"], "bob");

    // Agents lock for themselves only; tabs.lock on someone else's tab is refused.
    let err = w
        .call(agent("alice"), "tabs.lock", json!({ "tab": 1 }))
        .await
        .unwrap_err();
    assert_eq!(err.owner.as_deref(), Some("bob"));
    let err = w
        .call(
            agent("alice"),
            "tabs.lock",
            json!({ "tab": 1, "agent": "bob" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    // Unknown tab.
    let err = w
        .call(Caller::Ui, "tabs.unlock", json!({ "tab": 9 }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TabNotFound);
}

#[tokio::test]
async fn handoff_waits_for_the_user() {
    let w = world(&["https://a.test/"]);
    w.ok(agent("alice"), "nav.goto", goto(1)).await;
    let out = w
        .ok(
            agent("alice"),
            "ai.handoff",
            json!({ "reason": "Solve the CAPTCHA" }),
        )
        .await;
    assert_eq!(out["tab"], 1);
    assert_eq!(out["resolved"], false);
    assert_eq!(out["lock"]["state"], "handoff");
    assert_eq!(out["lock"]["reason"], "Solve the CAPTCHA");

    let agents = w.ok(Caller::Ui, "agents.list", json!({})).await;
    assert_eq!(agents["agents"][0]["status"], "waiting");
    assert_eq!(agents["agents"][0]["task"], "Solve the CAPTCHA");
    // Banner stays on: lockedBy is still the agent.
    assert_eq!(w.tab(1).await["lockedBy"], "alice");
    // The agent itself must wait.
    let err = w
        .call(agent("alice"), "nav.goto", goto(1))
        .await
        .unwrap_err();
    assert_eq!(err.owner.as_deref(), Some(USER));
    // The user may act in the tab without ending the handoff.
    w.ok(Caller::Ui, "nav.goto", goto(1)).await;
    assert_eq!(w.tab(1).await["lock"]["state"], "handoff");

    // Wait for "Done" in the background, then click it.
    let waiting = {
        let registry = w.registry.clone();
        let ctx = Ctx::new(agent("alice"), w.fake.clone(), w.bus.clone());
        tokio::spawn(registry.call(
            ctx,
            "ai.handoff",
            json!({ "tab": 1, "reason": "Log in", "wait_secs": 10 }),
        ))
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let out = w.ok(Caller::Ui, "tabs.unlock", json!({ "tab": 1 })).await;
    assert_eq!(out["lock"]["state"], "active");
    let done = waiting.await.unwrap().unwrap();
    assert_eq!(done["resolved"], true);
    w.ok(agent("alice"), "nav.goto", goto(1)).await;

    // Only agents hand off.
    let err = w
        .call(Caller::Rest, "ai.handoff", json!({ "reason": "x" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn agents_list_colors_forget_and_events() {
    let w = world(&["https://a.test/", "https://b.test/"]);
    let mut rx = w.bus.subscribe();
    w.ok(agent("alice"), "nav.goto", goto(1)).await;
    w.ok(
        Caller::Agent {
            id: "anthropic-1".into(),
        },
        "nav.goto",
        goto(2),
    )
    .await;

    let list = w.ok(Caller::Ui, "agents.list", json!({})).await;
    let agents = list["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["id"], "alice");
    assert_eq!(agents[0]["color"], "blue");
    assert_eq!(agents[0]["status"], "running"); // just called
    assert_eq!(agents[0]["tabs"], json!([1]));
    assert_eq!(agents[1]["id"], "anthropic-1");
    assert_eq!(agents[1]["color"], "orange");
    assert_eq!(agents[1]["groupId"], "agent:anthropic-1");

    // tab.updated with the lock fields, and agents.updated, were published.
    let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    let tab_event = events
        .iter()
        .find(|e| e.topic == "tab.updated" && e.data["id"] == 1)
        .unwrap();
    assert_eq!(tab_event.data["groupId"], "agent:alice");
    assert_eq!(tab_event.data["lockedBy"], "alice");
    assert!(events.iter().any(|e| e.topic == topics::AGENTS_UPDATED));

    // Forget releases the tabs; the color is free again for the next agent.
    let out = w
        .ok(Caller::Ui, "agents.forget", json!({ "id": "alice" }))
        .await;
    assert_eq!(out["released"], json!([1]));
    assert_eq!(w.tab(1).await["lock"], Value::Null);
    w.ok(agent("carol"), "nav.goto", goto(1)).await;
    let list = w.ok(Caller::Ui, "agents.list", json!({})).await;
    let carol = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "carol")
        .unwrap()
        .clone();
    assert_eq!(carol["color"], "blue");
    let err = w
        .call(Caller::Ui, "agents.forget", json!({ "id": "nobody" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn closed_tabs_leave_their_group() {
    let w = world(&["https://a.test/", "https://b.test/"]);
    w.ok(agent("alice"), "nav.goto", goto(1)).await;
    w.ok(agent("alice"), "tabs.close", json!({ "tab": 1 }))
        .await;
    let list = w.ok(Caller::Ui, "agents.list", json!({})).await;
    assert_eq!(list["agents"][0]["tabs"], json!([]));
    assert!(w.agents().tab_lock(TabId(1)).is_none());
}

#[test]
fn mcp_identity_from_client_name() {
    let agents = Agents::new();
    assert_eq!(agents.identify_mcp("s1", "claude-code"), "claude-code");
    // Same session → same id; another session with the same client → numbered.
    assert_eq!(agents.identify_mcp("s1", "claude-code"), "claude-code");
    assert_eq!(agents.identify_mcp("s2", "claude-code"), "claude-code-2");
    assert_eq!(
        agents.identify_mcp("s3", "Codex MCP Client"),
        "codex-mcp-client"
    );
    assert_eq!(slug("  ¿?  "), "agent");
}

#[test]
fn human_and_agent_callers() {
    assert!(Caller::Ui.is_human());
    assert!(Caller::Keymap.is_human());
    assert!(!Caller::Rest.is_human());
    assert_eq!(agent("a").agent_id(), Some("a"));
    assert_eq!(Caller::Agent { id: "b".into() }.agent_id(), Some("b"));
    assert_eq!(
        Caller::Mcp {
            session: "s".into(),
            agent: None
        }
        .agent_id(),
        None
    );
}
