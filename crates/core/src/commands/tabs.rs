//! `tabs.*` commands.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::backend::{NEW_TAB_URL, TabId, TabInfo};
use crate::command::{CmdError, Command, Ctx, Empty, Meta, Toolset};

/// `tabs.list`: all open tabs.
pub struct List;

/// A tab group (named, colored run of tabs). Not implemented yet: `tabs.list` always
/// returns an empty list, the type is here so the output schema is complete.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TabGroup {
    pub id: String,
    pub name: String,
    /// grey | blue | red | yellow | green | pink | purple | cyan | orange
    pub color: String,
    /// Set for agent-owned groups.
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListOutput {
    /// Open tabs in tab-strip order.
    pub tabs: Vec<TabInfo>,
    /// Tab groups (always empty for now).
    pub groups: Vec<TabGroup>,
    /// The tab the user is looking at, or null when no tab is open.
    pub active_id: Option<TabId>,
}

impl Command for List {
    const NAME: &'static str = "tabs.list";
    type Args = Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List open tabs in tab-strip order: id, title, url, loading, favicon, canGoBack, \
             canGoForward, active; plus activeId.",
            Toolset::Core,
        )
        .palette("List tabs")
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ListOutput, CmdError> {
        let tabs = ctx.backend.list_tabs().await?;
        let active_id = tabs.iter().find(|t| t.active).map(|t| t.id);
        Ok(ListOutput {
            tabs,
            groups: Vec::new(),
            active_id,
        })
    }
}

/// `tabs.open`: open a URL in a new tab.
pub struct Open;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// URL to open. Default: the new tab page.
    pub url: Option<String>,
    /// Open without switching to it (default false: the new tab becomes active).
    pub background: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OpenOutput {
    /// Id of the new tab (same as `id`; kept for older clients).
    pub tab: TabId,
    /// The new tab.
    #[serde(flatten)]
    pub info: TabInfo,
}

impl Command for Open {
    const NAME: &'static str = "tabs.open";
    type Args = OpenArgs;
    type Output = OpenOutput;

    fn meta() -> Meta {
        // Not `mutates_tab`: it creates a tab instead of changing an existing one.
        Meta::new(
            "Open a URL in a new tab (background: true keeps the current tab shown). Returns \
             the new tab; its id is in `tab` and `id`.",
            Toolset::Core,
        )
        .act()
        .palette("Open URL in new tab…")
    }

    async fn run(ctx: Ctx, args: OpenArgs) -> Result<OpenOutput, CmdError> {
        let url = args.url.unwrap_or_else(|| NEW_TAB_URL.to_owned());
        let background = args.background.unwrap_or(false);
        let tab = ctx.backend.open_tab(url.clone(), background).await?;
        // The browser exists now but may not have reported anything yet: fall back to
        // the requested URL.
        let info = ctx
            .backend
            .list_tabs()
            .await?
            .into_iter()
            .find(|t| t.id == tab)
            .unwrap_or_else(|| TabInfo::new(tab, url));
        Ok(OpenOutput { tab, info })
    }
}

/// Arguments of commands that act on exactly one tab.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabArgs {
    /// Tab id from tabs_list.
    pub tab: TabId,
}

/// `tabs.close`: close a tab.
pub struct Close;

impl Command for Close {
    const NAME: &'static str = "tabs.close";
    type Args = TabArgs;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new(
            "Close a tab. Closing the last tab of a window closes the window (and the last \
             window quits the browser).",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
        .palette("Close tab")
    }

    fn target_tab(args: &TabArgs) -> Option<TabId> {
        Some(args.tab)
    }

    async fn run(ctx: Ctx, args: TabArgs) -> Result<Empty, CmdError> {
        ctx.backend.close_tab(args.tab).await?;
        Ok(Empty {})
    }
}

/// `tabs.activate`: show a tab.
pub struct Activate;

impl Command for Activate {
    const NAME: &'static str = "tabs.activate";
    type Args = TabArgs;
    type Output = Empty;

    fn meta() -> Meta {
        // Not `mutates_tab`: switching tabs does not change any page.
        Meta::new(
            "Switch the window to this tab (make it the active, visible tab).",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: TabArgs) -> Result<Empty, CmdError> {
        ctx.backend.activate_tab(args.tab).await?;
        Ok(Empty {})
    }
}
