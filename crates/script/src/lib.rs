//! `bmcp-script`: runs user automation scripts (TypeScript) in an embedded V8
//! isolate via `deno_core`, in a SEPARATE process (`bmcp-script-host`) from the
//! CEF browser. libcef ships its own V8; keeping ours in another process avoids
//! symbol clashes and means a crashing script can't take the browser down.
//!
//! Flow of one browser call:
//!
//! ```text
//! script.ts: await browser.tabs.list()
//!   -> prelude.js Proxy: op_bmcp_call("tabs.list", null)
//!   -> ops.rs op_bmcp_call -> CommandSink::call
//!   -> MockSink (canned data) | WsSink (JSON-RPC over WebSocket to the browser)
//! ```
//!
//! Events flow the other way: `browser.on("tab.updated", h)` subscribes with
//! `events.subscribe`, and `op_bmcp_next_event` hands every event the sink receives to the
//! prelude, which calls the matching handlers. A script with handlers keeps running until the
//! connection closes, which is how plugins are long-lived (`crates/plugins`).
//!
//! Modules:
//! - [`host`]: create the isolate, run a script, enforce the timeout;
//! - [`loader`]: read + transpile TS (deno_ast) with source maps;
//! - [`ops`]: the few Rust functions JS can call + the JS prelude;
//! - [`sink`]: the `CommandSink` trait and `MockSink`;
//! - [`ws_sink`]: `WsSink`, JSON-RPC to the browser's `/v1/ws`.

pub mod host;
pub mod loader;
pub mod ops;
pub mod sink;
pub mod ws_sink;

pub use host::{RunOptions, RunOutcome, Timings, run_script};
pub use sink::{CallFuture, CommandSink, EventFuture, MockSink, SinkError};
pub use ws_sink::{DEFAULT_WS_URL, WsSink};
