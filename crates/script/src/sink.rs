//! Where browser commands go.
//!
//! A script calls `browser.tabs.list()`; the prelude turns that into
//! `op_bmcp_call("tabs.list", null)`; the op hands it to a [`CommandSink`].
//! The sink decides what "sending a command to the browser" means:
//! - [`MockSink`] answers from canned data (tests, offline runs);
//! - [`crate::ws_sink::WsSink`] sends JSON-RPC 2.0 over a WebSocket.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use deno_core::serde_json::{Value, json};

/// The future a sink returns. `Pin<Box<dyn Future>>` is the usual way to
/// return "some async work" from a trait method that must work behind
/// `dyn CommandSink`. It is NOT `Send`: everything runs on the one thread
/// that owns the V8 isolate.
pub type CallFuture = Pin<Box<dyn Future<Output = Result<Value, SinkError>>>>;

/// Anything that can execute a browser command by name.
pub trait CommandSink {
    /// Run command `name` (e.g. `"tabs.list"`) with JSON `args`
    /// (`null` when the script passed nothing) and return its JSON result.
    fn call(&self, name: String, args: Value) -> CallFuture;
}

/// A failed command. Mirrors a JSON-RPC error object (`code` + `message`).
#[derive(Debug, Clone)]
pub struct SinkError {
    pub code: Option<i64>,
    pub message: String,
}

impl SinkError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "{} (code {code})", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for SinkError {}

/// Fake browser with canned answers. No I/O at all.
///
/// - `tabs.list` returns two tabs;
/// - `test.fail` returns an error (to test error propagation into JS);
/// - anything else echoes `{ "called": name, "args": args }`.
#[derive(Debug, Default)]
pub struct MockSink;

impl CommandSink for MockSink {
    fn call(&self, name: String, args: Value) -> CallFuture {
        // `async move` builds the future; `Box::pin` puts it on the heap so it
        // fits the `CallFuture` type.
        Box::pin(async move {
            match name.as_str() {
                // Same shape as the real browser's `tabs.list`.
                "tabs.list" => Ok(json!({ "tabs": [
                    { "id": 1, "url": "https://example.com/", "title": "Example Domain", "loading": false },
                    { "id": 2, "url": "https://github.com/", "title": "GitHub", "loading": false }
                ] })),
                "test.fail" => Err(SinkError {
                    code: Some(-32000),
                    message: format!("mock failure, args={args}"),
                }),
                _ => Ok(json!({ "called": name, "args": args })),
            }
        })
    }
}
