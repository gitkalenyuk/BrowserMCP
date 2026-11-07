//! Commands that live in the API crate until their real home exists, plus `api.info` /
//! `api.config` (see [`crate::info`]).
//!
//! `page.ax_tree` is the spike-era accessibility preview, now in the hidden toolset: the
//! `perception`-based `page.snapshot` / `page.find` / `page.act` (crate `bmcp-browser`)
//! replace it.

use bmcp_core::{CmdError, Command, Ctx, Meta, Registry, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub fn register(registry: &mut Registry) {
    registry.add::<PageAxTree>();
    crate::info::register(registry);
}

pub struct PageAxTree;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageAxTreeArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Maximum number of lines (default 300).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PageAxTreeOutput {
    /// Tab that was read.
    pub tab: TabId,
    /// Indented `- role "name"` list.
    pub tree: String,
}

impl Command for PageAxTree {
    const NAME: &'static str = "page.ax_tree";
    type Args = PageAxTreeArgs;
    type Output = PageAxTreeOutput;

    fn meta() -> Meta {
        Meta::new(
            "Accessibility tree of the page as an indented list of roles and names (debug \
             preview; agents should use page_snapshot).",
            // Superseded by `page.snapshot` (bmcp-browser); kept for debugging only.
            Toolset::Hidden,
        )
    }

    async fn run(ctx: Ctx, args: Self::Args) -> Result<Self::Output, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let tree = ctx
            .backend
            .cdp(tab, "Accessibility.getFullAXTree".into(), json!({}))
            .await?;
        Ok(PageAxTreeOutput {
            tab,
            tree: crate::ax::format_ax_tree(&tree, args.limit.unwrap_or(300)),
        })
    }
}
