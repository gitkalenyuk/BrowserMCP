//! `nav.*` commands.

use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::backend::{TabId, TabInfo};
use crate::command::{CmdError, Command, Ctx, Meta, Toolset};

/// `nav.goto`: load a URL in a tab and wait for it.
pub struct Goto;

/// Upper bound for `timeout_secs`, so a typo cannot block a caller for hours.
const MAX_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GotoArgs {
    /// URL to load.
    pub url: String,
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Max seconds to wait for the page to finish loading (default 30, max 600).
    pub timeout_secs: Option<u64>,
}

impl Command for Goto {
    const NAME: &'static str = "nav.goto";
    type Args = GotoArgs;
    /// The tab after loading (its final URL and title).
    type Output = TabInfo;

    fn meta() -> Meta {
        Meta::new(
            "Navigate a tab to a URL and wait until the page has loaded. Returns the tab's \
             final url and title.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
        .palette("Go to URL…")
    }

    fn target_tab(args: &GotoArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: GotoArgs) -> Result<TabInfo, CmdError> {
        let timeout_secs = args.timeout_secs.unwrap_or(30);
        if timeout_secs == 0 || timeout_secs > MAX_TIMEOUT_SECS {
            return Err(CmdError::invalid_params(format!(
                "timeout_secs must be between 1 and {MAX_TIMEOUT_SECS}, got {timeout_secs}"
            )));
        }
        let tab = ctx.resolve_tab(args.tab).await?;
        let info = ctx
            .backend
            .navigate(tab, args.url, Duration::from_secs(timeout_secs))
            .await?;
        Ok(info)
    }
}
