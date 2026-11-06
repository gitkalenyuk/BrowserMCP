//! BrowserMCP recording (Phase 7): what happened in a tab → action log → TypeScript script,
//! plus video.
//!
//! * `record.start` / `record.stop` / `record.list` / `record.get` — the action log
//!   (`data/recordings/<id>/actions.jsonl`). Two sources:
//!   - API calls: every `page.*`, `nav.*`, `input.*`, `tabs.*` command from the event bus,
//!     with refs turned into role + name (see [`api`]);
//!   - people: a recorder script in an isolated world reports clicks, typing, keys, selects,
//!     checkboxes, scrolling; navigations come from CDP `Page.frameNavigated` (see [`human`]).
//!
//!   Trusted input sent by `page.act` is seen by both; the human copy is marked `dup_of`
//!   (see `session.rs`).
//! * `record.codegen` — the log as a TypeScript script for `bmcp-script-host` (see [`codegen`]).
//! * `record.video_start` / `record.video_stop` — CDP screencast → ffmpeg (see [`video`]).
//!
//! Events on the bus (a default WS topic, for the UI's Recording panel):
//! `record.started {id, tab, name, start_url}`, `record.step {id, step}` (each finished step,
//! secrets already redacted), `record.stopped {id, steps, video}`.
//!
//! No CEF dependency: everything goes through `BrowserBackend::cdp` / `cdp_events` and is
//! tested with `bmcp_core::testing::FakeBackend`.

pub mod api;
pub mod codegen;
pub mod commands;
pub mod human;
pub mod model;
mod session;
pub mod store;
#[cfg(test)]
mod tests;
pub mod video;

use bmcp_core::Registry;

pub use commands::EncoderOverride;
pub use store::DataDirExt;

/// Register the `record.*` commands and their shared state.
///
/// The app should also insert the data folder:
/// `registry.extensions().insert(bmcp_record::DataDirExt(data))`; without it recordings go to
/// the default `data/` folder next to the executable.
pub fn register(registry: &mut Registry) {
    registry.extensions().insert(store::Recordings::default());
    registry
        .add::<commands::Start>()
        .add::<commands::Stop>()
        .add::<commands::List>()
        .add::<commands::Get>()
        .add::<commands::Codegen>()
        .add::<commands::VideoStart>()
        .add::<commands::VideoStop>();
}
