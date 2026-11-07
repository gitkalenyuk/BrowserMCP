//! `tabs.*` commands.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agents::{Agents, TabGroup, TabLock};
use crate::backend::{ClosedTabInfo, NEW_TAB_URL, OpenTabOptions, TabId, TabInfo, WindowTarget};
use crate::command::{Caller, CmdError, Command, Ctx, Empty, Meta, Toolset};

/// `tabs.list`: all open tabs.
pub struct List;

/// One tab in `tabs.list`: the tab plus its agent lock.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ListedTab {
    #[serde(flatten)]
    pub info: TabInfo,
    /// Agent lock of the tab; null when no agent owns it.
    pub lock: Option<TabLock>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListOutput {
    /// Open tabs in tab-strip order; `groupId` / `lockedBy` / `lock` say which agent owns
    /// a tab.
    pub tabs: Vec<ListedTab>,
    /// Tab groups: one per known agent (`agent:<id>`).
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
             canGoForward, active, groupId/lockedBy/lock (agent ownership); plus groups and \
             activeId.",
            Toolset::Core,
        )
        .palette("List tabs")
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ListOutput, CmdError> {
        let tabs = ctx.backend.list_tabs().await?;
        let active_id = tabs.iter().find(|t| t.active).map(|t| t.id);
        let agents = Agents::from_ctx(&ctx);
        let open: Vec<TabId> = tabs.iter().map(|t| t.id).collect();
        agents.retain_open(&ctx.events, &open);
        let tabs = tabs
            .into_iter()
            .map(|mut info| {
                agents.decorate(&mut info);
                ListedTab {
                    lock: agents.tab_lock(info.id),
                    info,
                }
            })
            .collect();
        Ok(ListOutput {
            tabs,
            groups: agents.groups(),
            active_id,
        })
    }
}

/// Per-profile defaults of `tabs.open`. bmcp-profiles owns the profile settings
/// (profiles.json), which core does not know, so it provides them through the extensions
/// ([`ProfileTabDefaults`]).
pub trait TabDefaults: Send + Sync {
    /// Tabs of `profile` open in memory when the caller leaves `incognito` out.
    fn incognito_default(&self, profile: &str) -> bool;
}

/// The extension that holds the [`TabDefaults`] provider (absent = no defaults).
pub struct ProfileTabDefaults(pub Arc<dyn TabDefaults>);

/// `incognito` of a new tab: what the caller said, else the profile's default. Tabs opened
/// without a profile never become incognito by default (the browser's own profile keeps its
/// data, so a plain new tab never surprises).
fn incognito_for(ctx: &Ctx, incognito: Option<bool>, profile: Option<&str>) -> bool {
    match (incognito, profile) {
        (Some(incognito), _) => incognito,
        (None, Some(profile)) => ctx
            .extensions
            .get::<ProfileTabDefaults>()
            .is_some_and(|d| d.0.incognito_default(profile)),
        (None, None) => false,
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
    /// Profile to open it in (profile_list / profile_create): own cookies, cache, proxy and
    /// identity. Default: the profile the browser started with.
    pub profile: Option<String>,
    /// In-memory profile, nothing is written to disk. With `profile`: the in-memory profile
    /// of that name (shared by its tabs, gone when its last tab closes). Default: false, or
    /// the profile's `incognito_default` (profile_create) when `profile` is given.
    pub incognito: Option<bool>,
    /// "current" (default): a tab in the current window; "new": a window of its own;
    /// "offscreen": no visible window (automation; input and screenshots still work).
    pub window: Option<WindowTarget>,
    /// Mass run id this tab belongs to (set by run_start; downloads go to the run's folder).
    pub run: Option<String>,
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
            "Open a URL in a new tab (background: true keeps the current tab shown). Optional: \
             profile (own cookies/proxy/identity), incognito (in memory), window: \"new\" | \
             \"offscreen\" (own window / no window, for parallel automation). Returns the new \
             tab; its id is in `tab` and `id`.",
            Toolset::Core,
        )
        .act()
        .palette("Open URL in new tab…")
    }

    async fn run(ctx: Ctx, args: OpenArgs) -> Result<OpenOutput, CmdError> {
        let url = args.url.unwrap_or_else(|| NEW_TAB_URL.to_owned());
        let profile = args.profile.filter(|p| !p.is_empty());
        let options = OpenTabOptions {
            background: args.background.unwrap_or(false),
            incognito: incognito_for(&ctx, args.incognito, profile.as_deref()),
            profile,
            window: args.window.unwrap_or_default(),
            run: args.run.filter(|r| !r.is_empty()),
        };
        let tab = ctx.backend.open_tab_with(url.clone(), options).await?;
        // An agent owns the tabs it opens.
        ctx.tab_opened(tab);
        // The browser exists now but may not have reported anything yet: fall back to
        // the requested URL.
        let info = ctx
            .backend
            .list_tabs()
            .await?
            .into_iter()
            .find(|t| t.id == tab)
            .unwrap_or_else(|| TabInfo::new(tab, url));
        let mut info = info;
        Agents::from_ctx(&ctx).decorate(&mut info);
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
            "Close a tab. If it is the last tab of its window, the new tab page takes its \
             place and the window stays open (only a person closing the last tab closes the \
             window).",
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
        let keep_window = keeps_window(&ctx.caller);
        // Logged so an "unexplained" window close / quit can be traced to its caller.
        tracing::info!(tab = %args.tab, caller = ?ctx.caller, keep_window, "tabs.close");
        if keep_window {
            ctx.backend.close_tab_keep_window(args.tab).await?;
        } else {
            ctx.backend.close_tab(args.tab).await?;
        }
        Ok(Empty {})
    }
}

/// May closing a tab also close its window (when it is the last one)? Only when a person
/// does it (the browser UI, a keyboard shortcut). Agents, scripts, MCP/REST/WS clients and
/// runs get the new tab page in place of the last tab instead, so one automation cannot shut
/// down the browser everyone else is using.
pub fn keeps_window(caller: &Caller) -> bool {
    !matches!(caller, Caller::Ui | Caller::Keymap)
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

/// `tabs.split`: two tabs side by side in one window.
pub struct Split;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitArgs {
    /// Tab shown in the left half.
    pub left: TabId,
    /// Tab shown in the right half (same window as `left`).
    pub right: TabId,
    /// Width of the left half as a fraction of the page area, 0.15–0.85 (default 0.5).
    pub ratio: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SplitOutput {
    pub left: TabId,
    pub right: TabId,
    pub ratio: f64,
}

/// Split ratios outside this range leave one half too narrow to use.
pub const SPLIT_RATIO_RANGE: (f64, f64) = (0.15, 0.85);

/// `ratio` limited to [`SPLIT_RATIO_RANGE`] (missing / NaN = 0.5).
pub fn clamp_split_ratio(ratio: Option<f64>) -> f64 {
    match ratio {
        Some(r) if r.is_finite() => r.clamp(SPLIT_RATIO_RANGE.0, SPLIT_RATIO_RANGE.1),
        _ => 0.5,
    }
}

impl Command for Split {
    const NAME: &'static str = "tabs.split";
    type Args = SplitArgs;
    type Output = SplitOutput;

    fn meta() -> Meta {
        Meta::new(
            "Split view: show two tabs of the same window side by side (left | right, \
             ratio = width of the left half, default 0.5). Each half keeps its own tab; \
             activating either shows both. Calling it again changes the pair or the ratio.",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SplitArgs) -> Result<SplitOutput, CmdError> {
        if args.left == args.right {
            return Err(CmdError::invalid_params(
                "left and right must be different tabs",
            ));
        }
        let ratio = clamp_split_ratio(args.ratio);
        ctx.backend
            .split_tabs(args.left, args.right, Some(ratio))
            .await?;
        Ok(SplitOutput {
            left: args.left,
            right: args.right,
            ratio,
        })
    }
}

/// `tabs.unsplit`: end a split view.
pub struct Unsplit;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnsplitArgs {
    /// A tab of the split view to end. Default: the split view of the window used last.
    pub tab: Option<TabId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UnsplitOutput {
    /// The tabs that were split (empty when there was no split view).
    pub tabs: Vec<TabId>,
}

impl Command for Unsplit {
    const NAME: &'static str = "tabs.unsplit";
    type Args = UnsplitArgs;
    type Output = UnsplitOutput;

    fn meta() -> Meta {
        Meta::new(
            "End a split view: both tabs stay open, the focused one is shown alone.",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: UnsplitArgs) -> Result<UnsplitOutput, CmdError> {
        let tabs = ctx.backend.unsplit(args.tab).await?;
        Ok(UnsplitOutput { tabs })
    }
}

/// `tabs.restore_closed`: re-open the last closed tab (Ctrl+Shift+T).
pub struct RestoreClosed;

#[derive(Debug, Serialize, JsonSchema)]
pub struct RestoreClosedOutput {
    /// The re-opened tab; null when no closed tab is remembered.
    pub tab: Option<TabId>,
}

impl Command for RestoreClosed {
    const NAME: &'static str = "tabs.restore_closed";
    type Args = Empty;
    type Output = RestoreClosedOutput;

    fn meta() -> Meta {
        Meta::new(
            "Re-open the most recently closed tab (URL and profile, at its old place). \
             Returns null when nothing is left to restore.",
            Toolset::Core,
        )
        .act()
        .palette("Reopen closed tab")
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<RestoreClosedOutput, CmdError> {
        let tab = ctx.backend.restore_closed().await?;
        if let Some(tab) = tab {
            ctx.tab_opened(tab);
        }
        Ok(RestoreClosedOutput { tab })
    }
}

/// `tabs.closed`: the recently closed tabs (Settings → Startup & tabs, Ctrl+Shift+T order).
pub struct Closed;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClosedOutput {
    /// Most recent first (at most 25; incognito and off-screen tabs are never listed).
    pub tabs: Vec<ClosedTabInfo>,
}

impl Command for Closed {
    const NAME: &'static str = "tabs.closed";
    type Args = Empty;
    type Output = ClosedOutput;

    fn meta() -> Meta {
        Meta::new(
            "List the recently closed tabs, most recent first: index, url, title, profile. \
             Reopen one with tabs_closed_reopen {index}.",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ClosedOutput, CmdError> {
        Ok(ClosedOutput {
            tabs: ctx.backend.closed_tabs().await?,
        })
    }
}

/// `tabs.closed_reopen`: reopen one entry of `tabs.closed`.
pub struct ClosedReopen;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClosedReopenArgs {
    /// `index` from tabs_closed (0 = the tab closed last).
    pub index: usize,
}

impl Command for ClosedReopen {
    const NAME: &'static str = "tabs.closed_reopen";
    type Args = ClosedReopenArgs;
    type Output = RestoreClosedOutput;

    fn meta() -> Meta {
        Meta::new(
            "Reopen entry `index` of tabs_closed (URL and profile, at its old place) and remove \
             it from the list. Returns the new tab, or null when there is no such entry.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ClosedReopenArgs) -> Result<RestoreClosedOutput, CmdError> {
        let tab = ctx.backend.restore_closed_at(args.index).await?;
        if let Some(tab) = tab {
            ctx.tab_opened(tab);
        }
        Ok(RestoreClosedOutput { tab })
    }
}

/// `tabs.closed_clear`: forget the recently closed tabs.
pub struct ClosedClear;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClosedClearOutput {
    /// How many entries were forgotten.
    pub cleared: usize,
}

impl Command for ClosedClear {
    const NAME: &'static str = "tabs.closed_clear";
    type Args = Empty;
    type Output = ClosedClearOutput;

    fn meta() -> Meta {
        Meta::new(
            "Forget the recently closed tabs (Ctrl+Shift+T then has nothing to reopen). \
             Returns how many were forgotten.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ClosedClearOutput, CmdError> {
        Ok(ClosedClearOutput {
            cleared: ctx.backend.clear_closed().await?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::keeps_window;

    #[test]
    fn split_ratio_is_clamped() {
        use super::clamp_split_ratio;
        assert_eq!(clamp_split_ratio(None), 0.5);
        assert_eq!(clamp_split_ratio(Some(f64::NAN)), 0.5);
        assert_eq!(clamp_split_ratio(Some(0.3)), 0.3);
        assert_eq!(clamp_split_ratio(Some(0.01)), 0.15);
        assert_eq!(clamp_split_ratio(Some(2.0)), 0.85);
    }
    use crate::command::Caller;

    #[test]
    fn only_people_may_close_the_last_window_through_tabs_close() {
        // A person: closing the last tab closes the window (normal browser behaviour).
        assert!(!keeps_window(&Caller::Ui));
        assert!(!keeps_window(&Caller::Keymap));
        // Automation: the new tab page replaces the last tab, the window stays.
        for caller in [
            Caller::Rest,
            Caller::Ws { conn: 1 },
            Caller::Mcp {
                session: "s".into(),
                agent: None,
            },
            Caller::Script { id: "x".into() },
            Caller::Agent { id: "a".into() },
            Caller::Job {
                run: "r".into(),
                step: 1,
            },
        ] {
            assert!(keeps_window(&caller), "{caller:?}");
        }
    }
}
