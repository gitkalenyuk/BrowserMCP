//! BrowserMCP core.
//!
//! This crate must never depend on CEF: everything here is plain Rust and unit-testable.
//!
//! * [`backend`] — the `BrowserBackend` trait the CEF crate implements;
//! * [`command`] — the `Command` trait and the `Registry` every API surface is generated from;
//! * [`commands`] — the built-in commands (`tabs.list`, `nav.goto`, …);
//! * [`events`] — the `EventBus` (command started/finished, later page and network events);
//! * [`extensions`] — typed map of shared services that outlive one call (per-tab state, …);
//! * [`paths`] — portable data directory layout.

pub mod backend;
pub mod command;
pub mod commands;
pub mod events;
pub mod extensions;
pub mod paths;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use backend::{
    BackendError, BackendResult, BoxFuture, BrowserBackend, CdpEvent, CdpEvents, TabId, TabInfo,
};
pub use command::{
    Caller, CmdError, Command, CommandInfo, Ctx, ErrorCode, LockPolicy, Meta, Registry, Retry,
    Sensitivity, Toolset, wire_name,
};
pub use events::{Event, EventBus};
pub use extensions::Extensions;
pub use paths::{DataDir, PathError, ProfileId};
