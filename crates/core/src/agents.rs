//! [`Agents`]: who the AI agents are, which tabs they own, and the tab locks
//! (plan: "Кілька ШІ — кожен агент — своя кольорова група вкладок + lock; людина може
//! перехопити").
//!
//! **Identity.** Every caller that is an agent gets an [`AgentId`]: MCP clients from the
//! `?agent=` URL parameter or, without it, from their `clientInfo.name` (see
//! [`Agents::identify_mcp`]); the built-in AI from `Caller::Agent { id }`. An agent is
//! remembered from its first call until `agents.forget`, with a stable color (the first one
//! no other agent uses).
//!
//! **Ownership.** The first time an agent mutates a tab (or opens one) the tab joins the
//! agent's colored group (`agent:<id>`) and is locked to it ([`LockState::Active`]).
//!
//! **Rules** (one table, implemented in [`decide`]):
//!
//! | tab state            | owner agent | other agent        | human (UI/keys)      | REST/WS/script  |
//! |----------------------|-------------|--------------------|----------------------|-----------------|
//! | free                 | claims it   | claims it          | ok                   | ok              |
//! | `active` (agent's)   | ok          | `TAB_LOCKED{owner}`| ok + **takeover** → `paused` | `TAB_LOCKED` |
//! | `paused` (user's)    | `TAB_LOCKED{user}` | `TAB_LOCKED{user}` | ok             | ok              |
//! | `handoff` (waiting)  | `TAB_LOCKED{user}` | `TAB_LOCKED{owner}`| ok             | `TAB_LOCKED`    |
//!
//! Transitions outside commands' lock checks: `tabs.unlock` by a human pauses an `active`
//! tab ("Take over") and resolves a `handoff` back to `active`; `tabs.lock` by a human gives
//! a `paused` tab back to its agent; `tabs.unlock` by the owner agent releases the tab;
//! `ai.handoff` puts the agent's tab into `handoff` ("waiting for you: <reason>").
//!
//! Changes are published as `tab.updated {id, groupId, lockedBy, lock}` and
//! `agents.updated {agents}` on the event bus (shapes of `ui/src/lib/api/types.ts`).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::{TabId, TabInfo};
use crate::command::{Caller, CmdError, Ctx, ErrorCode, LockPolicy};
use crate::events::EventBus;

/// Agent ids are plain strings: `bench`, `claude-code`, `claude-code-1`, `anthropic-2`.
pub type AgentId = String;

/// Event topics of this module.
pub mod topics {
    /// `agents.updated {agents: [AgentInfo]}` — the whole list, after any change.
    pub const AGENTS_UPDATED: &str = "agents.updated";
    /// `ai.handoff {tab, agent, reason}` — an agent asks the user to do something in a tab.
    pub const HANDOFF: &str = "ai.handoff";
}

/// `"user"`: the owner reported in `TAB_LOCKED` when a human controls the tab.
pub const USER: &str = "user";

/// Group colors handed out in this order (the UI's `GroupColor` names; `grey` is left for
/// human groups).
pub const COLORS: [&str; 8] = [
    "blue", "orange", "green", "purple", "pink", "cyan", "yellow", "red",
];

/// After this long without calls an agent counts as `idle`.
const IDLE_AFTER: Duration = Duration::from_secs(5);

/// Who controls an agent-owned tab right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    /// The owner agent drives the tab; everyone else except humans is refused.
    Active,
    /// A human took over; agents are refused until the human gives it back (`tabs.lock`).
    Paused,
    /// The agent handed the tab to the human (`ai.handoff`) and waits for "Done"
    /// (`tabs.unlock`), which makes it `active` again.
    Handoff,
}

/// Lock of one tab, as listed in `tabs.list` (`lock`) and sent in `tab.updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TabLock {
    /// Agent whose group the tab is in.
    pub owner: AgentId,
    pub state: LockState,
    /// `handoff`: what the agent asks the user to do.
    pub reason: Option<String>,
    /// When the state last changed, ms since the Unix epoch.
    pub since_ms: u64,
}

/// What an agent is doing, in the UI's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Running,
    /// Waiting for the user (a tab in `handoff`).
    Waiting,
    Error,
}

/// One agent as `agents.list` returns it (the UI's `Agent` type plus a few extras).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: AgentId,
    /// Display name, e.g. "claude-code" or "Claude Code".
    pub name: String,
    /// Group color name: blue | orange | green | purple | pink | cyan | yellow | red.
    pub color: String,
    pub status: AgentStatus,
    /// What it is doing right now (last command, or the handoff reason).
    pub task: Option<String>,
    /// Its tab group id (`agent:<id>`).
    pub group_id: String,
    /// Tabs in its group.
    pub tabs: Vec<TabId>,
    /// Number of commands it called.
    pub calls: u64,
    /// Last call, ms since the Unix epoch.
    pub last_seen_ms: u64,
}

/// A tab group owned by an agent (the UI's `TabGroup`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TabGroup {
    pub id: String,
    pub name: String,
    /// grey | blue | red | yellow | green | pink | purple | cyan | orange
    pub color: String,
    /// Set for agent-owned groups.
    pub agent_id: Option<AgentId>,
}

/// Group id of an agent's tabs.
pub fn group_id(agent: &str) -> String {
    format!("agent:{agent}")
}

/// Turn a client name ("Claude Code", "codex-mcp-client") into an id: lowercase ASCII
/// letters/digits, everything else `-`, at most 40 characters.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(40).collect();
    if out.is_empty() { "agent".into() } else { out }
}

struct Agent {
    name: String,
    color: &'static str,
    in_flight: u32,
    calls: u64,
    task: Option<String>,
    last_seen_ms: u64,
}

struct Owned {
    lock: TabLock,
    /// Claim order; the newest claimed tab is the agent's default tab.
    seq: u64,
}

#[derive(Default)]
struct Inner {
    agents: BTreeMap<AgentId, Agent>,
    tabs: BTreeMap<TabId, Owned>,
    /// MCP session id → agent id (agents identified by client name).
    sessions: HashMap<String, AgentId>,
    next_seq: u64,
}

/// What a state change needs to announce (emitted after the mutex is released).
#[derive(Default)]
struct Changes {
    tabs: Vec<TabId>,
    agents: bool,
}

/// The agent registry and lock policy. One per browser, shared as `Arc<Agents>`: it is the
/// registry's [`LockPolicy`] and lives in its `Extensions` for the `agents.*` / `tabs.lock`
/// commands.
#[derive(Default)]
pub struct Agents {
    inner: Mutex<Inner>,
}

impl Agents {
    pub fn new() -> Self {
        Self::default()
    }

    /// The service from a command's context.
    pub fn from_ctx(ctx: &Ctx) -> Arc<Agents> {
        // Installed by `register_builtin`; a bare registry gets a private, empty one.
        ctx.extensions.get_or_insert_with(Agents::new)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Every update is applied in one go, so a poisoned mutex is still consistent.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ------------------------------------------------------------------ identity

    /// Agent id for an MCP session that did not pass `?agent=`: the client's name
    /// (`claude-code`), or `claude-code-2`, `-3`… when another session already uses it.
    pub fn identify_mcp(&self, session: &str, client_name: &str) -> AgentId {
        let mut inner = self.lock();
        if let Some(id) = inner.sessions.get(session) {
            return id.clone();
        }
        let base = slug(client_name);
        let taken = |inner: &Inner, id: &str| inner.sessions.values().any(|s| s == id);
        let mut id = base.clone();
        let mut n = 2;
        while taken(&inner, &id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        inner.sessions.insert(session.to_owned(), id.clone());
        id
    }

    /// Make sure agent `id` is known (with `name` when given) and return its info.
    /// Used by the built-in AI to pick the session color before the first call.
    pub fn ensure(&self, events: &EventBus, id: &str, name: Option<&str>) -> AgentInfo {
        let (info, new) = {
            let mut inner = self.lock();
            let new = touch(&mut inner, id);
            if let (Some(name), Some(agent)) = (name, inner.agents.get_mut(id)) {
                agent.name = name.to_owned();
            }
            (agent_info(&inner, id).expect("just added"), new)
        };
        if new {
            self.emit(
                events,
                Changes {
                    tabs: vec![],
                    agents: true,
                },
            );
        }
        info
    }

    /// Every known agent, oldest first.
    pub fn list(&self) -> Vec<AgentInfo> {
        let inner = self.lock();
        inner
            .agents
            .keys()
            .filter_map(|id| agent_info(&inner, id))
            .collect()
    }

    /// Forget an agent: its tabs leave its group and are unlocked. Returns the released tabs,
    /// or `None` for an unknown id.
    pub fn forget(&self, events: &EventBus, id: &str) -> Option<Vec<TabId>> {
        let tabs = {
            let mut inner = self.lock();
            inner.agents.remove(id)?;
            inner.sessions.retain(|_, agent| agent != id);
            let tabs: Vec<TabId> = owned_tabs(&inner, id);
            for tab in &tabs {
                inner.tabs.remove(tab);
            }
            tabs
        };
        self.emit(
            events,
            Changes {
                tabs: tabs.clone(),
                agents: true,
            },
        );
        Some(tabs)
    }

    // ------------------------------------------------------------------ tabs

    /// The lock of `tab`, if an agent owns it.
    pub fn tab_lock(&self, tab: TabId) -> Option<TabLock> {
        self.lock().tabs.get(&tab).map(|o| o.lock.clone())
    }

    /// One group per known agent (also agents without tabs, so the UI knows the group before
    /// the first `tab.updated` puts a tab in it).
    pub fn groups(&self) -> Vec<TabGroup> {
        let inner = self.lock();
        inner
            .agents
            .iter()
            .map(|(id, a)| TabGroup {
                id: group_id(id),
                name: a.name.clone(),
                color: a.color.into(),
                agent_id: Some(id.clone()),
            })
            .collect()
    }

    /// Fill `groupId` / `lockedBy` of a tab from the lock table.
    pub fn decorate(&self, tab: &mut TabInfo) {
        let lock = self.tab_lock(tab.id);
        tab.group_id = lock.as_ref().map(|l| group_id(&l.owner));
        tab.locked_by = lock.as_ref().and_then(locked_by);
    }

    /// Drop locks of tabs that are no longer open (called by commands that list tabs).
    pub fn retain_open(&self, events: &EventBus, open: &[TabId]) {
        let gone = {
            let mut inner = self.lock();
            let gone: Vec<TabId> = inner
                .tabs
                .keys()
                .filter(|t| !open.contains(t))
                .copied()
                .collect();
            for tab in &gone {
                inner.tabs.remove(tab);
            }
            gone
        };
        if !gone.is_empty() {
            // The tabs are closed: only the agents' tab lists changed.
            self.emit(
                events,
                Changes {
                    tabs: vec![],
                    agents: true,
                },
            );
        }
    }

    /// `tabs.lock`: `caller` (an agent) claims `tab` for itself, or a human / other caller
    /// gives it (back) to `agent` (default: the agent whose group the tab is in).
    pub fn lock_tab(
        &self,
        ctx: &Ctx,
        tab: TabId,
        agent: Option<&str>,
    ) -> Result<TabLock, CmdError> {
        let result = {
            let mut inner = self.lock();
            let mut changes = Changes::default();
            match ctx.caller.agent_id() {
                Some(me) => {
                    if agent.is_some_and(|a| a != me) {
                        return Err(CmdError::invalid_params(
                            "an agent can only lock tabs for itself; omit `agent`",
                        ));
                    }
                    decide(inner.tabs.get(&tab).map(|o| &o.lock), &ctx.caller, tab)?;
                    claim(&mut inner, me, tab, &mut changes);
                }
                None => {
                    let owner = agent
                        .map(str::to_owned)
                        .or_else(|| inner.tabs.get(&tab).map(|o| o.lock.owner.clone()))
                        .ok_or_else(|| {
                            CmdError::invalid_params(format!(
                                "tab {tab} is not in an agent group; pass `agent`"
                            ))
                            .with_hint("agents_list shows the agent ids.")
                        })?;
                    if !inner.agents.contains_key(&owner) {
                        return Err(CmdError::new(
                            ErrorCode::NotFound,
                            format!("unknown agent '{owner}'"),
                        )
                        .with_hint("agents_list shows the agent ids."));
                    }
                    let seq = next_seq(&mut inner);
                    inner.tabs.insert(
                        tab,
                        Owned {
                            lock: new_lock(&owner, LockState::Active, None),
                            seq,
                        },
                    );
                    changes.tabs.push(tab);
                    changes.agents = true;
                }
            }
            (inner.tabs[&tab].lock.clone(), changes)
        };
        self.emit(&ctx.events, result.1);
        Ok(result.0)
    }

    /// `tabs.unlock`: a human (or REST/WS caller) takes over an `active` tab or finishes a
    /// `handoff`; the owner agent releases its tab. Returns the new lock (`None` = free).
    pub fn unlock_tab(&self, ctx: &Ctx, tab: TabId) -> Result<Option<TabLock>, CmdError> {
        let result = {
            let mut inner = self.lock();
            let Some(owned) = inner.tabs.get(&tab) else {
                return Ok(None); // nothing to unlock
            };
            let owner = owned.lock.owner.clone();
            let state = owned.lock.state;
            match ctx.caller.agent_id() {
                Some(me) if me == owner => {
                    inner.tabs.remove(&tab);
                }
                Some(_) => return Err(locked_error(tab, &owned.lock)),
                None => {
                    let next = match state {
                        LockState::Active => LockState::Paused,
                        LockState::Handoff => LockState::Active,
                        LockState::Paused => LockState::Paused,
                    };
                    set_state(&mut inner, tab, next, None);
                }
            }
            (
                inner.tabs.get(&tab).map(|o| o.lock.clone()),
                Changes {
                    tabs: vec![tab],
                    agents: true,
                },
            )
        };
        self.emit(&ctx.events, result.1);
        Ok(result.0)
    }

    /// `ai.handoff`: the calling agent hands `tab` to the user ("waiting for you").
    pub fn handoff(&self, ctx: &Ctx, tab: TabId, reason: &str) -> Result<TabLock, CmdError> {
        let Some(me) = ctx.caller.agent_id() else {
            return Err(CmdError::invalid_params(
                "only agents hand tabs to the user (MCP clients and the built-in AI)",
            ));
        };
        let lock = {
            let mut inner = self.lock();
            let mut changes = Changes::default();
            let current = inner.tabs.get(&tab).map(|o| &o.lock);
            match current {
                // Already waiting: just update the reason.
                Some(l) if l.owner == me && l.state == LockState::Handoff => {}
                _ => decide(current, &ctx.caller, tab)?,
            }
            claim(&mut inner, me, tab, &mut changes);
            set_state(&mut inner, tab, LockState::Handoff, Some(reason.to_owned()));
            inner.tabs[&tab].lock.clone()
        };
        self.emit(
            &ctx.events,
            Changes {
                tabs: vec![tab],
                agents: true,
            },
        );
        ctx.events.emit(
            topics::HANDOFF,
            json!({ "tab": tab, "agent": me, "reason": reason }),
        );
        Ok(lock)
    }

    // ------------------------------------------------------------------ events

    /// `tab.updated {id, groupId, lockedBy, lock}` for one tab.
    pub fn tab_event(&self, tab: TabId) -> Value {
        let lock = self.tab_lock(tab);
        tab_payload(tab, lock.as_ref())
    }

    fn emit(&self, events: &EventBus, changes: Changes) {
        for tab in changes.tabs {
            events.emit(crate::events::topics::TAB_UPDATED, self.tab_event(tab));
        }
        if changes.agents {
            events.emit(topics::AGENTS_UPDATED, json!({ "agents": self.list() }));
        }
    }
}

/// `tab.updated` payload with only the lock fields (the UI merges it into the tab).
pub fn tab_payload(tab: TabId, lock: Option<&TabLock>) -> Value {
    json!({
        "id": tab,
        "groupId": lock.map(|l| group_id(&l.owner)),
        "lockedBy": lock.and_then(locked_by),
        "lock": lock,
    })
}

/// `lockedBy` for the UI: the agent while it controls (or waits on) the tab, null once the
/// human has taken over.
fn locked_by(lock: &TabLock) -> Option<AgentId> {
    (lock.state != LockState::Paused).then(|| lock.owner.clone())
}

/// The lock table rule for one mutating call (see the module docs).
fn decide(lock: Option<&TabLock>, caller: &Caller, tab: TabId) -> Result<(), CmdError> {
    let Some(lock) = lock else { return Ok(()) };
    if caller.is_human() {
        return Ok(());
    }
    match (caller.agent_id(), lock.state) {
        (Some(me), LockState::Active) if me == lock.owner => Ok(()),
        // REST, WS, scripts and jobs act for the user once the user has the tab.
        (None, LockState::Paused) => Ok(()),
        _ => Err(locked_error(tab, lock)),
    }
}

fn locked_error(tab: TabId, lock: &TabLock) -> CmdError {
    match lock.state {
        LockState::Paused => CmdError::new(
            ErrorCode::TabLocked,
            format!("the user took over tab {tab}"),
        )
        .with_owner(USER)
        .with_hint(
            "The user is using this tab. Open your own tab with tabs_open, or wait \
                     until the user gives it back.",
        ),
        LockState::Handoff => CmdError::new(
            ErrorCode::TabLocked,
            format!(
                "tab {tab} is waiting for the user: {}",
                lock.reason.as_deref().unwrap_or("handoff")
            ),
        )
        .with_owner(USER)
        .with_hint("Wait until the user clicks Done in the browser, then retry."),
        LockState::Active => CmdError::new(
            ErrorCode::TabLocked,
            format!("tab {tab} is locked by agent {}", lock.owner),
        )
        .with_owner(lock.owner.clone())
        .with_hint("Another agent works in this tab. Use your own tab (tabs_open) or wait."),
    }
}

/// Add agent `id` if it is new; returns true when it was.
fn touch(inner: &mut Inner, id: &str) -> bool {
    if inner.agents.contains_key(id) {
        return false;
    }
    let used: Vec<&str> = inner.agents.values().map(|a| a.color).collect();
    // First free color; with more than 8 agents, cycle through them.
    let color = COLORS
        .iter()
        .find(|c| !used.contains(c))
        .copied()
        .unwrap_or(COLORS[inner.agents.len() % COLORS.len()]);
    inner.agents.insert(
        id.to_owned(),
        Agent {
            name: id.to_owned(),
            color,
            in_flight: 0,
            calls: 0,
            task: None,
            last_seen_ms: now_ms(),
        },
    );
    true
}

/// Put a free tab into agent `id`'s group (no-op when it already owns it).
fn claim(inner: &mut Inner, id: &str, tab: TabId, changes: &mut Changes) {
    if inner.tabs.get(&tab).is_some_and(|o| o.lock.owner == id) {
        return;
    }
    if inner.tabs.contains_key(&tab) {
        return; // someone else's: `decide` refused it already
    }
    touch(inner, id);
    let seq = next_seq(inner);
    inner.tabs.insert(
        tab,
        Owned {
            lock: new_lock(id, LockState::Active, None),
            seq,
        },
    );
    changes.tabs.push(tab);
    changes.agents = true;
}

fn set_state(inner: &mut Inner, tab: TabId, state: LockState, reason: Option<String>) {
    if let Some(owned) = inner.tabs.get_mut(&tab) {
        owned.lock = new_lock(&owned.lock.owner, state, reason);
    }
}

fn new_lock(owner: &str, state: LockState, reason: Option<String>) -> TabLock {
    TabLock {
        owner: owner.to_owned(),
        state,
        reason,
        since_ms: now_ms(),
    }
}

fn next_seq(inner: &mut Inner) -> u64 {
    inner.next_seq += 1;
    inner.next_seq
}

fn owned_tabs(inner: &Inner, id: &str) -> Vec<TabId> {
    inner
        .tabs
        .iter()
        .filter(|(_, o)| o.lock.owner == id)
        .map(|(t, _)| *t)
        .collect()
}

fn agent_info(inner: &Inner, id: &str) -> Option<AgentInfo> {
    let agent = inner.agents.get(id)?;
    let tabs = owned_tabs(inner, id);
    let handoff = inner
        .tabs
        .values()
        .find(|o| o.lock.owner == id && o.lock.state == LockState::Handoff);
    let recent = now_ms().saturating_sub(agent.last_seen_ms) < IDLE_AFTER.as_millis() as u64;
    let (status, task) = if let Some(h) = handoff {
        (AgentStatus::Waiting, h.lock.reason.clone())
    } else if agent.in_flight > 0 || recent {
        (AgentStatus::Running, agent.task.clone())
    } else {
        (AgentStatus::Idle, None)
    };
    Some(AgentInfo {
        id: id.to_owned(),
        name: agent.name.clone(),
        color: agent.color.into(),
        status,
        task,
        group_id: group_id(id),
        tabs,
        calls: agent.calls,
        last_seen_ms: agent.last_seen_ms,
    })
}

impl LockPolicy for Agents {
    fn check(&self, caller: &Caller, tab: Option<TabId>) -> Result<(), CmdError> {
        let Some(tab) = tab else { return Ok(()) };
        let inner = self.lock();
        decide(inner.tabs.get(&tab).map(|o| &o.lock), caller, tab)
    }

    /// Check, then: an agent claims a free tab; a human acting on an agent's active tab
    /// takes it over (the agent's lock pauses).
    fn acquire(&self, ctx: &Ctx, tab: Option<TabId>) -> Result<(), CmdError> {
        let Some(tab) = tab else { return Ok(()) };
        let changes = {
            let mut inner = self.lock();
            let current = inner.tabs.get(&tab).map(|o| &o.lock);
            decide(current, &ctx.caller, tab)?;
            let mut changes = Changes::default();
            if let Some(me) = ctx.caller.agent_id() {
                claim(&mut inner, me, tab, &mut changes);
            } else if ctx.caller.is_human() && current.is_some_and(|l| l.state == LockState::Active)
            {
                set_state(&mut inner, tab, LockState::Paused, None);
                changes.tabs.push(tab);
                changes.agents = true;
            }
            changes
        };
        self.emit(&ctx.events, changes);
        Ok(())
    }

    fn opened(&self, ctx: &Ctx, tab: TabId) {
        let Some(me) = ctx.caller.agent_id() else {
            return;
        };
        let mut changes = Changes::default();
        claim(&mut self.lock(), me, tab, &mut changes);
        self.emit(&ctx.events, changes);
    }

    fn default_tab(&self, caller: &Caller) -> Option<TabId> {
        let me = caller.agent_id()?;
        let inner = self.lock();
        inner
            .tabs
            .iter()
            .filter(|(_, o)| o.lock.owner == me)
            .max_by_key(|(_, o)| o.seq)
            .map(|(t, _)| *t)
    }

    fn started(&self, ctx: &Ctx, name: &str) {
        let Some(me) = ctx.caller.agent_id() else {
            return;
        };
        let announce = {
            let mut inner = self.lock();
            let new = touch(&mut inner, me);
            let before = agent_info(&inner, me).map(|a| a.status);
            let agent = inner.agents.get_mut(me).expect("touched");
            agent.in_flight += 1;
            agent.calls += 1;
            agent.task = Some(name.to_owned());
            agent.last_seen_ms = now_ms();
            new || before != agent_info(&inner, me).map(|a| a.status)
        };
        if announce {
            self.emit(
                &ctx.events,
                Changes {
                    tabs: vec![],
                    agents: true,
                },
            );
        }
    }

    fn finished(&self, ctx: &Ctx, _name: &str, _ok: bool) {
        let Some(me) = ctx.caller.agent_id() else {
            return;
        };
        let idle_now = {
            let mut inner = self.lock();
            let Some(agent) = inner.agents.get_mut(me) else {
                return;
            };
            agent.in_flight = agent.in_flight.saturating_sub(1);
            agent.last_seen_ms = now_ms();
            agent.in_flight == 0
        };
        // Announce `idle` once the agent has been quiet for IDLE_AFTER (needs a runtime;
        // without one the next `agents.list` still shows the right status).
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if !idle_now {
            return;
        }
        let Some(this) = ctx.extensions.get::<Agents>() else {
            return;
        };
        let (events, me) = (ctx.events.clone(), me.to_owned());
        runtime.spawn(async move {
            tokio::time::sleep(IDLE_AFTER + Duration::from_millis(100)).await;
            let idle = this
                .list()
                .iter()
                .any(|a| a.id == me && a.status == AgentStatus::Idle);
            if idle {
                this.emit(
                    &events,
                    Changes {
                        tabs: vec![],
                        agents: true,
                    },
                );
            }
        });
    }
}

#[cfg(test)]
mod tests;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
