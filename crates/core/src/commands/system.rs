//! `system.*` commands.

use schemars::JsonSchema;
use serde::Serialize;

use crate::command::{CmdError, Command, Ctx, Empty, Meta, Toolset};

/// `system.ping`: is BrowserMCP alive, and which version.
pub struct Ping;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PingOutput {
    /// Always "BrowserMCP".
    pub name: String,
    /// BrowserMCP version, e.g. "0.1.0".
    pub version: String,
}

impl Command for Ping {
    const NAME: &'static str = "system.ping";
    type Args = Empty;
    type Output = PingOutput;

    fn meta() -> Meta {
        Meta::new(
            "Check that BrowserMCP is running. Returns its name and version.",
            Toolset::Core,
        )
    }

    async fn run(_ctx: Ctx, _args: Empty) -> Result<PingOutput, CmdError> {
        Ok(PingOutput {
            name: "BrowserMCP".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }
}
