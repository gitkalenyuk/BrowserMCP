//! Commands: `tabs.freeze` (toolset `core`), `governor.status`, `governor.config`,
//! `governor.recommend_parallel` (toolset `system`).

use bmcp_core::{CmdError, Command, Ctx, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{GovernorConfig, Limits, Profile};
use crate::memory::{Recommendation, recommend_parallel};
use crate::service::{FreezeMode, Governor, Outcome, Status};

/// Default memory per parallel slot for `governor.recommend_parallel` (script host + its tabs).
pub const DEFAULT_SLOT_MB: u64 = 400;

// ------------------------------------------------------------------ tabs.freeze

pub struct Freeze;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FreezeArgs {
    /// Tab id from tabs_list.
    pub tab: TabId,
    /// freeze = pause the page (keeps memory, no CPU); discard = unload it and keep URL,
    /// title and scroll to restore (frees memory); thaw = undo either.
    pub mode: FreezeMode,
}

impl Command for Freeze {
    const NAME: &'static str = "tabs.freeze";
    type Args = FreezeArgs;
    type Output = Outcome;

    fn meta() -> Meta {
        Meta::new(
            "Save resources on a background tab: mode freeze pauses it, discard unloads it \
             (restored on activation or with mode thaw), thaw resumes/reloads it. Active, \
             locked, recorded, recently used and audible tabs are refused. Returns the new \
             state.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &FreezeArgs) -> Option<TabId> {
        Some(args.tab)
    }

    async fn run(ctx: Ctx, args: FreezeArgs) -> Result<Outcome, CmdError> {
        let gov = Governor::shared(&ctx.extensions);
        gov.apply(&*ctx.backend, &ctx.events, args.tab, args.mode, "manual")
            .await
    }
}

// ------------------------------------------------------------------ governor.status

pub struct StatusCmd;

impl Command for StatusCmd {
    const NAME: &'static str = "governor.status";
    type Args = bmcp_core::command::Empty;
    type Output = Status;

    fn meta() -> Meta {
        Meta::new(
            "Resource governor state: limits in force (profile from RAM + overrides), RAM and \
             the browser's process tree working set, every tab's state \
             (live/frozen/discarded, idle time, why it is protected, JS heap) and the recent \
             decisions.",
            Toolset::System,
        )
        .palette("Governor: memory and tab states")
    }

    async fn run(ctx: Ctx, _args: bmcp_core::command::Empty) -> Result<Status, CmdError> {
        Governor::shared(&ctx.extensions)
            .status(&*ctx.backend)
            .await
    }
}

// ------------------------------------------------------------------ governor.config

pub struct Config;

/// Every field is optional; only the given ones change. 0 for a time or max_live_tabs = off.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigArgs {
    /// Clear all overrides first (back to the profile's defaults).
    pub reset: Option<bool>,
    /// Turn the automatic policy on/off (manual tabs.freeze always works).
    pub enabled: Option<bool>,
    /// low | normal | high; default chosen from RAM.
    pub profile: Option<Profile>,
    /// Freeze after this many seconds in the background.
    pub freeze_after_secs: Option<u64>,
    /// Discard after this many seconds in the background.
    pub discard_after_secs: Option<u64>,
    /// Discard the most idle tabs above this many live (not discarded) tabs.
    pub max_live_tabs: Option<usize>,
    /// Memory pressure below this percentage of free RAM.
    pub pressure_free_pct: Option<u8>,
    /// A tab that got a command this recently is never touched.
    pub busy_secs: Option<u64>,
    /// Policy interval.
    pub tick_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigOutput {
    /// The limits in force now.
    pub limits: Limits,
    /// What is stored in data/config/governor.json.
    pub overrides: GovernorConfig,
}

impl Command for Config {
    const NAME: &'static str = "governor.config";
    type Args = ConfigArgs;
    type Output = ConfigOutput;

    fn meta() -> Meta {
        Meta::new(
            "Change the resource governor's limits (saved in data/config/governor.json): \
             enabled, profile, freeze_after_secs, discard_after_secs, max_live_tabs, \
             pressure_free_pct, busy_secs, tick_secs; reset clears overrides. Returns the \
             limits in force.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ConfigArgs) -> Result<ConfigOutput, CmdError> {
        let patch = GovernorConfig {
            enabled: args.enabled,
            profile: args.profile,
            freeze_after_secs: args.freeze_after_secs,
            discard_after_secs: args.discard_after_secs,
            max_live_tabs: args.max_live_tabs,
            pressure_free_pct: args.pressure_free_pct,
            busy_secs: args.busy_secs,
            tick_secs: args.tick_secs,
        };
        let gov = Governor::shared(&ctx.extensions);
        let limits = gov
            .configure(&patch, args.reset.unwrap_or(false))
            .map_err(|e| CmdError::internal(format!("cannot save governor.json: {e}")))?;
        Ok(ConfigOutput {
            limits,
            overrides: gov.overrides(),
        })
    }
}

// ------------------------------------------------------------------ governor.recommend_parallel

pub struct RecommendParallel;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecommendArgs {
    /// Memory one parallel slot needs (default 400 MB: a script host plus its tabs).
    pub per_slot_mb: Option<u64>,
}

impl Command for RecommendParallel {
    const NAME: &'static str = "governor.recommend_parallel";
    type Args = RecommendArgs;
    type Output = Recommendation;

    fn meta() -> Meta {
        Meta::new(
            "How many things to run in parallel on this machine: min(CPUs / 2, available RAM \
             / per_slot_mb), at least 1. The mass-run auto-limit uses the same rule.",
            Toolset::System,
        )
    }

    async fn run(_ctx: Ctx, args: RecommendArgs) -> Result<Recommendation, CmdError> {
        let mb = args.per_slot_mb.unwrap_or(DEFAULT_SLOT_MB).max(1);
        Ok(recommend_parallel(mb * 1024 * 1024))
    }
}
