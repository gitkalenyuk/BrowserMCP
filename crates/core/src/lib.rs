//! BrowserMCP core.
//!
//! This crate must never depend on CEF: everything here is plain Rust and unit-testable.
//! Later phases add the command registry, event bus and domain model here.

pub mod paths;

pub use paths::{DataDir, PathError, ProfileId};
