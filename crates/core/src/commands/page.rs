//! `page.*` commands.
//!
//! `page.snapshot` and the other perception-based commands live in the `bmcp-browser` crate;
//! the raw `Accessibility.getFullAXTree` passthrough is not ported.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::backend::{BackendError, TabId, ZoomChange};
use crate::command::{CmdError, Command, Ctx, Meta, Toolset};
use crate::zoom;

/// `page.text`: visible text of the page.
pub struct Text;

const DEFAULT_LIMIT: usize = 8000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Maximum number of characters to return (default 8000).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TextOutput {
    /// The tab that was read.
    pub tab: TabId,
    /// `document.body.innerText`, cut to `limit` characters.
    pub text: String,
    /// True when the text was longer than `limit`.
    pub truncated: bool,
    /// Length of the full text in characters.
    pub total_chars: usize,
}

impl Command for Text {
    const NAME: &'static str = "page.text";
    type Args = TextArgs;
    type Output = TextOutput;

    fn meta() -> Meta {
        Meta::new(
            "Visible text of the page (document.body.innerText), cut to `limit` characters.",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: TextArgs) -> Result<TextOutput, CmdError> {
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
        let tab = ctx.resolve_tab(args.tab).await?;
        // TODO(stealth): evaluate in an isolated world instead of the page's main world.
        let reply = ctx
            .backend
            .cdp(
                tab,
                "Runtime.evaluate".into(),
                json!({
                    "expression": "document.body ? document.body.innerText : ''",
                    "returnByValue": true,
                }),
            )
            .await?;
        if let Some(details) = reply.get("exceptionDetails") {
            return Err(CmdError::internal(format!(
                "reading the page text failed: {}",
                details["text"].as_str().unwrap_or("JavaScript exception")
            )));
        }

        let full = reply["result"]["value"].as_str().unwrap_or_default();
        let total_chars = full.chars().count();
        Ok(TextOutput {
            tab,
            text: full.chars().take(limit).collect(),
            truncated: total_chars > limit,
            total_chars,
        })
    }
}

/// `page.zoom`: read or change a tab's zoom.
pub struct Zoom;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoomArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// New zoom in percent, 25-500 (100 = normal size).
    pub level: Option<f64>,
    /// Zoom presets to step: +1 = zoom in (Ctrl+=), -1 = zoom out (Ctrl+-), 0 = reset
    /// (Ctrl+0). Ignored when `level` is given. Without both: just report the zoom.
    pub delta: Option<i32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ZoomOutput {
    pub tab: TabId,
    /// Zoom in percent after the change.
    pub level: f64,
}

impl Command for Zoom {
    const NAME: &'static str = "page.zoom";
    type Args = ZoomArgs;
    type Output = ZoomOutput;

    fn meta() -> Meta {
        Meta::new(
            "Page zoom of a tab in percent: level sets it (25-500), delta steps through \
             Chrome's presets (+1 in, -1 out, 0 reset); without both it only reports the \
             zoom. Remembered per host.",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ZoomArgs) -> Result<ZoomOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let change = match (args.level, args.delta) {
            (Some(level), _) => {
                let range = zoom::MIN_PERCENT..=zoom::MAX_PERCENT;
                if !level.is_finite() || !range.contains(&level) {
                    return Err(CmdError::invalid_params("level must be 25-500 (percent)")
                        .with_hint("100 is normal size; use delta: 1 / -1 / 0 to step."));
                }
                ZoomChange::Percent(level)
            }
            (None, Some(delta)) => ZoomChange::Steps(delta.clamp(-20, 20)),
            // Only report: the zoom the tab shows now.
            (None, None) => {
                let tabs = ctx.backend.list_tabs().await?;
                let info = tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .ok_or(BackendError::TabNotFound(tab))?;
                let level = info.zoom.map_or(100.0, f64::from);
                return Ok(ZoomOutput { tab, level });
            }
        };
        let level = ctx.backend.zoom(tab, change).await?;
        Ok(ZoomOutput { tab, level })
    }
}
