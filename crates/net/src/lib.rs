//! BrowserMCP traffic (Phase 6): see, change and reuse what a tab sends and receives.
//!
//! * `net.capture` — record a tab's requests through CDP `Network.*` into a ring buffer
//!   (last 5000 requests; headers, status, timing, initiator, size, WebSocket frames);
//! * `net.list` / `net.get` / `net.search` / `net.ws_frames` — compact views, with cursors;
//!   bodies only on request and cut to `max_bytes`;
//! * `net.har_export` — HAR 1.2 file (data/har/) or inline;
//! * `net.rule_add` / `net.rule_list` / `net.rule_remove` — block, redirect, header changes,
//!   body rewrites and mocks through CDP `Fetch.*` (enabled only while rules exist);
//! * `net.breakpoint` / `net.paused_list` / `net.resume` — hold requests like Burp's
//!   intercept; `net.paused` event per pause; auto-continue after 60 s;
//! * `net.replay` — send a captured request again from the page;
//! * `net.api_to_code` — a typed TypeScript client from the captured XHR/fetch calls.
//!
//! Everything goes through `BrowserBackend::cdp` / `cdp_events`, so there is no CEF
//! dependency and the tests use `bmcp_core::testing::FakeBackend`. All commands are in
//! `Toolset::Net`: MCP clients ask for them with `/mcp?toolsets=core,net`.
//!
//! Module map: [`store`] (the data, built from events) · [`listener`] (background task per
//! tab) · [`rules`] + [`intercept`] (Fetch handling) · [`state`] (per-tab state in
//! `Extensions`) · [`format`], [`har`], [`apicode`] (output) · `commands` / `rule_commands`.

pub mod apicode;
pub mod commands;
pub mod format;
pub mod har;
pub mod intercept;
pub mod listener;
pub mod rule_commands;
pub mod rules;
pub mod state;
pub mod store;
#[cfg(test)]
mod tests;

use bmcp_core::Registry;

pub use state::NetState;

/// Register the `net.*` commands and their shared state.
pub fn register(registry: &mut Registry) {
    registry.extensions().insert(NetState::default());
    registry
        .add::<commands::Capture>()
        .add::<commands::List>()
        .add::<commands::Get>()
        .add::<commands::Search>()
        .add::<commands::WsFrames>()
        .add::<commands::HarExport>()
        .add::<commands::ApiToCode>()
        .add::<rule_commands::RuleAdd>()
        .add::<rule_commands::RuleList>()
        .add::<rule_commands::RuleRemove>()
        .add::<rule_commands::Breakpoint>()
        .add::<rule_commands::PausedList>()
        .add::<rule_commands::Resume>()
        .add::<rule_commands::Replay>();
}
