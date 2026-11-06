//! BrowserMCP core.
//!
//! This crate must never depend on CEF: everything here is plain Rust and unit-testable.
//! Later phases add the command registry, event bus and domain model here.

pub mod backend;
pub mod paths;

pub use backend::{BackendError, BackendResult, BoxFuture, BrowserBackend, TabId, TabInfo};
pub use paths::{DataDir, PathError, ProfileId};
