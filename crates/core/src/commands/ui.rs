//! `ui.*` commands: plumbing for the browser's own Svelte chrome.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::command::{CmdError, Command, Ctx, Empty, Meta, Toolset};

/// `ui.layout`: the chrome reports where its page area is.
///
/// Accepted and ignored over the API. The CEF build places the page from the
/// `bmcp://cmd/layout` message instead, because only that channel knows which window the
/// report came from (see `crates/cef/src/chrome/protocol.rs`).
pub struct Layout;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LayoutArgs {
    /// Left edge of the page area, CSS px relative to the UI view.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// `devicePixelRatio` of the UI view.
    pub dpr: Option<f64>,
}

impl Command for Layout {
    const NAME: &'static str = "ui.layout";
    type Args = LayoutArgs;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new(
            "Internal: the browser UI reports its page area. No effect over the API.",
            Toolset::Hidden,
        )
    }

    async fn run(_ctx: Ctx, _args: LayoutArgs) -> Result<Empty, CmdError> {
        Ok(Empty {})
    }
}
