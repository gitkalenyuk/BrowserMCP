//! BrowserMCP core.
//!
//! This crate must never depend on CEF: everything here is plain Rust and unit-testable.
//!
//! * [`agents`] — agent identity, tab groups and locks (the real `LockPolicy`);
//! * [`backend`] — the `BrowserBackend` trait the CEF crate implements;
//! * [`cdp`] — what the app did with the CDP port (never combined with a stealth identity);
//! * [`command`] — the `Command` trait and the `Registry` every API surface is generated from;
//! * [`commands`] — the built-in commands (`tabs.list`, `nav.goto`, …);
//! * [`config_file`] — small JSON settings files in `data/config/` (atomic writes);
//! * [`events`] — the `EventBus` (command started/finished, `tab.*`, later page and network events);
//! * [`extensions`] — typed map of shared services that outlive one call (per-tab state, …);
//! * [`paths`] — portable data directory layout;
//! * [`tabs`] — `TabStrip`, the tab order + active tab of one window.

pub mod agents;
pub mod backend;
pub mod cdp;
pub mod command;
pub mod commands;
pub mod config_file;
pub mod events;
pub mod extensions;
pub mod paths;
pub mod tabs;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod zoom;

pub use agents::{AgentId, Agents};
pub use backend::{
    BackendError, BackendResult, BoxFuture, BrowserBackend, CdpEvent, CdpEvents, ClosedTabInfo,
    NEW_TAB_URL, OpenTabOptions, SplitSide, TabId, TabInfo, TabSplit, WindowTarget, ZoomChange,
};
pub use cdp::CdpLaunch;
pub use command::{
    Caller, CmdError, Command, CommandGuard, CommandInfo, Ctx, ErrorCode, LockPolicy, Meta,
    Registry, Retry, Sensitivity, Toolset, wire_name,
};
pub use events::{Event, EventBus};
pub use extensions::Extensions;
pub use paths::{DataDir, PathError, ProfileId};
pub use tabs::TabStrip;
