//! BrowserMCP resource saving (plan Phase 4: M4.6 adblock, M4.8 reader/simplify).
//!
//! * [`engine`] — [`RequestFilter`]: Brave's `adblock` engine (EasyList syntax): `check`
//!   (block / redirect / important) and `cosmetic` (hide selectors, CSS, scriptlets);
//! * [`lists`] — filter lists in `data/filters/`: download with ETag/Last-Modified, compile,
//!   cache the compiled engine in `engine.dat`;
//! * [`policy`] — per-site simplification (`data/config/simplify.json`: js, images, fonts,
//!   media, third_party, animations) and the adblock allowlist (`data/config/adblock.json`);
//! * [`gate`] — [`ResourceGate`]/[`Gate`]: the one per-request decision, CEF-agnostic, built
//!   for CEF's IO thread (`thread_local!` engine per thread). The CEF hook is specified in
//!   `crates/adblock/README.md`;
//! * [`interim`] — INTERIM enforcement over CDP `Fetch` (`adblock.apply`) until that hook lands;
//! * [`reader`] — reader mode with Mozilla Readability in an isolated world.
//!
//! Commands (see [`commands`]): `adblock.update_lists`, `adblock.toggle`, `adblock.stats`,
//! `adblock.check`, `adblock.apply`, `simplify.site`, `simplify.get`, `simplify.list`,
//! `simplify.measure` (toolset `data`) and `simplify.reader` (toolset `core`).

pub mod commands;
pub mod engine;
pub mod gate;
pub mod interim;
pub mod lists;
pub mod measure;
pub mod policy;
pub mod reader;
pub mod service;
pub mod stats;
#[cfg(test)]
mod tests;

use bmcp_core::Registry;

pub use engine::{CosmeticResources, Decision, RequestFilter, ResourceType};
pub use gate::{Gate, GateAction, GateDecision, Reason, RequestInfo, ResourceGate};
pub use policy::{ResolvedPolicy, SitePolicy};
pub use service::AdblockService;

/// Register the commands. The shared [`AdblockService`] is created on first use, under the
/// `bmcp_core::DataDir` found in the registry's extensions (put one there at startup).
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::UpdateLists>()
        .add::<commands::Toggle>()
        .add::<commands::Stats>()
        .add::<commands::Check>()
        .add::<commands::Apply>()
        .add::<commands::Site>()
        .add::<commands::Get>()
        .add::<commands::List>()
        .add::<measure::Measure>()
        .add::<commands::Reader>();
}
