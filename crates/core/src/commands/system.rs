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

/// `system.quit`: shut BrowserMCP down gracefully.
pub struct Quit;

#[derive(Debug, Serialize, JsonSchema)]
pub struct QuitOutput {
    /// Always true: the browser is closing (shortly after this answer).
    pub quitting: bool,
}

/// Time for the answer to reach the caller before the API goes away with the browser.
const QUIT_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

impl Command for Quit {
    const NAME: &'static str = "system.quit";
    type Args = Empty;
    type Output = QuitOutput;

    fn meta() -> Meta {
        Meta::new(
            "Quit BrowserMCP: close every tab and window, stop the API and end the process \
             (the way to stop a --headless browser, which outlives its last tab). Answers \
             first, then quits.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<QuitOutput, CmdError> {
        let backend = ctx.backend.clone();
        tokio::spawn(async move {
            tokio::time::sleep(QUIT_DELAY).await;
            if let Err(err) = backend.quit().await {
                tracing::warn!(%err, "system.quit failed");
            }
        });
        Ok(QuitOutput { quitting: true })
    }
}
