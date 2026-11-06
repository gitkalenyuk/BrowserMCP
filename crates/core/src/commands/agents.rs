//! `agents.*`, `tabs.lock` / `tabs.unlock` / `tabs.groups` and `ai.handoff`: the commands of
//! the multi-agent model in [`crate::agents`].

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agents::{AgentId, AgentInfo, Agents, LockState, TabGroup, TabLock};
use crate::backend::TabId;
use crate::command::{CmdError, Command, Ctx, Empty, ErrorCode, Meta, Toolset};

/// Tab ids that are open right now; also drops locks of closed tabs.
async fn prune(ctx: &Ctx, agents: &Agents) -> Result<Vec<TabId>, CmdError> {
    let open: Vec<TabId> = ctx
        .backend
        .list_tabs()
        .await?
        .iter()
        .map(|t| t.id)
        .collect();
    agents.retain_open(&ctx.events, &open);
    Ok(open)
}

async fn require_open(ctx: &Ctx, agents: &Agents, tab: TabId) -> Result<(), CmdError> {
    if prune(ctx, agents).await?.contains(&tab) {
        Ok(())
    } else {
        Err(CmdError::from(crate::backend::BackendError::TabNotFound(
            tab,
        )))
    }
}

// ---------------------------------------------------------------- agents.list / forget

/// `agents.list`
pub struct List;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// Known agents, oldest first.
    pub agents: Vec<AgentInfo>,
}

impl Command for List {
    const NAME: &'static str = "agents.list";
    type Args = Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List AI agents that used the browser: id, name, color, status \
             (idle|running|waiting), current task, their tab group and tabs.",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ListOutput, CmdError> {
        let agents = Agents::from_ctx(&ctx);
        prune(&ctx, &agents).await?;
        Ok(ListOutput {
            agents: agents.list(),
        })
    }
}

/// `agents.forget`
pub struct Forget;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForgetArgs {
    /// Agent id from agents_list.
    pub id: AgentId,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ForgetOutput {
    /// Tabs that left the agent's group and were unlocked.
    pub released: Vec<TabId>,
}

impl Command for Forget {
    const NAME: &'static str = "agents.forget";
    type Args = ForgetArgs;
    type Output = ForgetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Forget an agent: unlock its tabs, remove its tab group and color. It is \
             registered again on its next call.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ForgetArgs) -> Result<ForgetOutput, CmdError> {
        let agents = Agents::from_ctx(&ctx);
        let released = agents.forget(&ctx.events, &args.id).ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, format!("unknown agent '{}'", args.id))
                .with_hint("agents_list shows the agent ids.")
        })?;
        Ok(ForgetOutput { released })
    }
}

// ---------------------------------------------------------------- tabs.lock / unlock / groups

/// Output of `tabs.lock` / `tabs.unlock`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LockOutput {
    pub tab: TabId,
    /// The tab's lock now; null = no agent owns the tab.
    pub lock: Option<TabLock>,
}

/// `tabs.lock`
pub struct Lock;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LockArgs {
    /// Tab id from tabs_list.
    pub tab: TabId,
    /// Only for the user / scripts: give the tab to this agent (default: the agent whose
    /// group the tab is in). Agents leave it out and lock for themselves.
    pub agent: Option<AgentId>,
}

impl Command for Lock {
    const NAME: &'static str = "tabs.lock";
    type Args = LockArgs;
    type Output = LockOutput;

    fn meta() -> Meta {
        Meta::new(
            "Lock a tab for yourself so other agents get TAB_LOCKED (tabs you open or act on \
             are locked automatically). The user can give a taken-over tab back with it.",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: LockArgs) -> Result<LockOutput, CmdError> {
        let agents = Agents::from_ctx(&ctx);
        require_open(&ctx, &agents, args.tab).await?;
        let lock = agents.lock_tab(&ctx, args.tab, args.agent.as_deref())?;
        Ok(LockOutput {
            tab: args.tab,
            lock: Some(lock),
        })
    }
}

/// `tabs.unlock`
pub struct Unlock;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnlockArgs {
    /// Tab id from tabs_list.
    pub tab: TabId,
}

impl Command for Unlock {
    const NAME: &'static str = "tabs.unlock";
    type Args = UnlockArgs;
    type Output = LockOutput;

    fn meta() -> Meta {
        Meta::new(
            "Agents: release your tab (it leaves your group). The user: take over an agent's \
             tab (its lock pauses), or finish a handoff (the agent continues).",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: UnlockArgs) -> Result<LockOutput, CmdError> {
        let agents = Agents::from_ctx(&ctx);
        require_open(&ctx, &agents, args.tab).await?;
        let lock = agents.unlock_tab(&ctx, args.tab)?;
        Ok(LockOutput {
            tab: args.tab,
            lock,
        })
    }
}

/// `tabs.groups`
pub struct Groups;

/// A group with the tabs in it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GroupWithTabs {
    #[serde(flatten)]
    pub group: TabGroup,
    pub tabs: Vec<TabId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GroupsOutput {
    pub groups: Vec<GroupWithTabs>,
}

impl Command for Groups {
    const NAME: &'static str = "tabs.groups";
    type Args = Empty;
    type Output = GroupsOutput;

    fn meta() -> Meta {
        Meta::new(
            "Tab groups (one colored group per agent) with their tab ids.",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<GroupsOutput, CmdError> {
        let agents = Agents::from_ctx(&ctx);
        prune(&ctx, &agents).await?;
        let list = agents.list();
        let groups = agents
            .groups()
            .into_iter()
            .map(|group| {
                let tabs = list
                    .iter()
                    .find(|a| Some(&a.id) == group.agent_id.as_ref())
                    .map(|a| a.tabs.clone())
                    .unwrap_or_default();
                GroupWithTabs { group, tabs }
            })
            .collect();
        Ok(GroupsOutput { groups })
    }
}

// ---------------------------------------------------------------- ai.handoff

/// `ai.handoff`
pub struct Handoff;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffArgs {
    /// Tab the user should work in. Default: your current tab.
    pub tab: Option<TabId>,
    /// What the user should do, shown in the browser, e.g. "Solve the CAPTCHA, then click Done".
    pub reason: String,
    /// Wait up to this many seconds for the user to click Done (0 = return at once,
    /// default 0, max 600).
    pub wait_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HandoffOutput {
    pub tab: TabId,
    /// True when the user clicked Done (the tab is yours again).
    pub resolved: bool,
    pub lock: Option<TabLock>,
}

impl Command for Handoff {
    const NAME: &'static str = "ai.handoff";
    type Args = HandoffArgs;
    type Output = HandoffOutput;

    fn meta() -> Meta {
        Meta::new(
            "Ask the user to do something in a tab (log in, CAPTCHA, confirm). The browser \
             shows a banner with `reason`; when the user clicks Done the tab is yours again. \
             Use wait_secs to wait for that.",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: HandoffArgs) -> Result<HandoffOutput, CmdError> {
        let agents = Agents::from_ctx(&ctx);
        let tab = ctx.resolve_tab(args.tab).await?;
        require_open(&ctx, &agents, tab).await?;
        agents.handoff(&ctx, tab, &args.reason)?;

        // Poll for "Done": simple and cheap (one mutex read per 200 ms).
        let wait = Duration::from_secs(args.wait_secs.unwrap_or(0).min(600));
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let lock = agents.tab_lock(tab);
            let waiting = lock.as_ref().is_some_and(|l| l.state == LockState::Handoff);
            if !waiting || tokio::time::Instant::now() >= deadline {
                return Ok(HandoffOutput {
                    tab,
                    resolved: !waiting,
                    lock,
                });
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
