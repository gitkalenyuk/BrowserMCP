//! `tabs.*` commands.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::backend::{TabId, TabInfo};
use crate::command::{CmdError, Command, Ctx, Empty, Meta, Toolset};

/// `tabs.list`: all open tabs.
pub struct List;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// Open tabs, oldest first.
    pub tabs: Vec<TabInfo>,
}

impl Command for List {
    const NAME: &'static str = "tabs.list";
    type Args = Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List open tabs (oldest first): id, title, url, loading.",
            Toolset::Core,
        )
        .palette("List tabs")
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ListOutput, CmdError> {
        Ok(ListOutput {
            tabs: ctx.backend.list_tabs().await?,
        })
    }
}

/// `tabs.open`: open a URL in a new tab.
pub struct Open;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// URL to open in a new tab.
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OpenOutput {
    /// Id of the new tab.
    pub tab: TabId,
}

impl Command for Open {
    const NAME: &'static str = "tabs.open";
    type Args = OpenArgs;
    type Output = OpenOutput;

    fn meta() -> Meta {
        // Not `mutates_tab`: it creates a tab instead of changing an existing one.
        Meta::new(
            "Open a URL in a new tab. Returns the new tab id.",
            Toolset::Core,
        )
        .act()
        .palette("Open URL in new tab…")
    }

    async fn run(ctx: Ctx, args: OpenArgs) -> Result<OpenOutput, CmdError> {
        let tab = ctx.backend.open_tab(args.url).await?;
        Ok(OpenOutput { tab })
    }
}
