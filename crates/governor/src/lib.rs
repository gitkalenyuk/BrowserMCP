//! BrowserMCP resource governor (plan: "Економія: заморожування/вивантаження вкладок,
//! авто-ліміти від RAM/CPU"; "Залізо: різне → автоматичні ліміти").
//!
//! * [`actions`] — the CDP sequences: freeze/thaw (`Page.setWebLifecycleState`), discard
//!   (remember URL/title/scroll, load a `data:` placeholder) and restore;
//! * [`policy`] — the automatic policy as a pure function (idle time, max live tabs, memory
//!   pressure → what to freeze/discard);
//! * [`config`] — profiles chosen from RAM (low ≤ 8 GB, normal ~16 GB, high ≥ 32 GB) and the
//!   overrides in `data/config/governor.json`;
//! * [`memory`] — RAM, the working set of our process tree, the parallelism recommendation
//!   used by mass runs;
//! * [`service`] — [`Governor`]: per-tab state, [`TabGuard`]s, the decision log, the
//!   background service ([`start`]).
//!
//! Commands: `tabs.freeze {tab, mode: freeze|discard|thaw}` (toolset `core`),
//! `governor.status`, `governor.config {…}`, `governor.recommend_parallel {per_slot_mb}`
//! (toolset `system`).
//!
//! Events: `tab.frozen {id, reason}`, `tab.thawed {id, reason}`,
//! `tab.discarded {id, url, title, reason}`, `tab.restored {id, url, reason}`.
//!
//! Never touched: the active tab, tabs locked by an agent (`TabInfo::locked_by`), tabs being
//! recorded (`record.started/stopped` on the bus), tabs that got a command in the last
//! `busy_secs` (agents, scripts, mass-run jobs), tabs playing audio, and whatever a registered
//! [`TabGuard`] protects. A frozen/discarded tab wakes up when it is activated or when a
//! command targets it.
//!
//! No CEF dependency: everything goes through `BrowserBackend` and is tested with
//! `bmcp_core::testing::FakeBackend`.

pub mod actions;
pub mod commands;
pub mod config;
pub mod memory;
pub mod policy;
pub mod service;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use bmcp_core::{BrowserBackend, EventBus, Extensions, Registry};

pub use config::{GovernorConfig, Limits, Profile};
pub use memory::{MemInfo, MemorySource, Recommendation, SystemMemory, recommend_parallel};
pub use service::{FreezeMode, Governor, TabGuard, TabState};

/// Event topics emitted on the bus (all under the UI's default `tab.*` subscription).
pub mod topics {
    pub const TAB_FROZEN: &str = "tab.frozen";
    pub const TAB_THAWED: &str = "tab.thawed";
    pub const TAB_DISCARDED: &str = "tab.discarded";
    pub const TAB_RESTORED: &str = "tab.restored";
}

/// Register the commands. The shared [`Governor`] is created on first use.
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::Freeze>()
        .add::<commands::StatusCmd>()
        .add::<commands::Config>()
        .add::<commands::RecommendParallel>();
}

/// Start the background service (automatic policy + wake-ups) on the current tokio runtime.
/// Call it once, after the `DataDir` is in `ext` (so `governor.json` is found).
pub fn start(
    ext: &Extensions,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
) -> Arc<Governor> {
    let gov = Governor::shared(ext);
    tokio::spawn(service::run(gov.clone(), backend, events));
    gov
}
