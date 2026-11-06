//! `page.*` commands.
//!
//! TODO(perception): `page.snapshot` / `page.ax_tree` come from the perception crate that is
//! being built in parallel; the raw `Accessibility.getFullAXTree` passthrough is not ported.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::backend::TabId;
use crate::command::{CmdError, Command, Ctx, Meta, Toolset};

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
