//! BrowserMCP page control: the commands an AI agent uses to see and drive a page without
//! screenshots.
//!
//! * `page.snapshot` / `page.find` — compact text with refs (`@4:e3`) from `bmcp-perception`;
//! * `page.act` / `page.act_batch` — click, fill, select… by ref, with actionability checks
//!   and a one-line diff of what changed;
//! * `page.diff`, `page.read`, `page.screenshot` (only when text is not enough);
//! * `nav.back`, `nav.forward`, `nav.reload`, `nav.wait`, `input.keys`.
//!
//! Everything goes through `BrowserBackend::cdp`, so this crate has no CEF dependency and is
//! tested with `bmcp_core::testing::FakeBackend`. Per-tab state (ref registry, recent
//! snapshots) lives in [`PerceptionState`] inside the registry's `Extensions`.

mod act;
mod cdp;
pub mod commands;
mod keys;
mod page;
mod state;
#[cfg(test)]
mod tests;

use bmcp_core::Registry;

pub use act::{Action, Step};
pub use state::{PerceptionState, TabState};

/// Register this crate's commands and its shared state.
pub fn register(registry: &mut Registry) {
    registry.extensions().insert(PerceptionState::default());
    registry
        .add::<commands::Snapshot>()
        .add::<commands::Find>()
        .add::<commands::Act>()
        .add::<commands::ActBatch>()
        .add::<commands::Diff>()
        .add::<commands::Read>()
        .add::<commands::Screenshot>()
        .add::<commands::Back>()
        .add::<commands::Forward>()
        .add::<commands::Reload>()
        .add::<commands::Wait>()
        .add::<commands::Keys>();
}
