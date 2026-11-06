//! Taking snapshots and waiting for pages: the glue between CDP and `bmcp-perception`.

use std::time::Duration;

use bmcp_core::{CmdError, Ctx, ErrorCode, Retry, TabId, TabInfo};
use bmcp_perception::{
    AxTree, Mode, PageHeader, PerceptionError, Scroll, Snapshot, SnapshotOptions, Viewport,
    snapshot,
};
use serde_json::json;

use crate::cdp::{self, perception_error};
use crate::state::{PerceptionState, TabState};

/// Longest we wait for a page to finish loading after an action.
pub const SETTLE_MAX: Duration = Duration::from_secs(10);
/// Pause right after an action, so the page can react (start a navigation, re-render).
const SETTLE_FIRST_PAUSE: Duration = Duration::from_millis(150);
/// If no navigation started within this time after the action, we stop waiting for one.
const SETTLE_NAV_GRACE: Duration = Duration::from_millis(400);
const POLL: Duration = Duration::from_millis(100);

/// The tab's entry from `list_tabs`.
pub async fn tab_info(ctx: &Ctx, tab: TabId) -> Result<TabInfo, CmdError> {
    ctx.backend
        .list_tabs()
        .await?
        .into_iter()
        .find(|t| t.id == tab)
        .ok_or_else(|| bmcp_core::BackendError::TabNotFound(tab).into())
}

/// Check whether the tab shows a new document since we last looked (main-frame `loaderId`
/// changed). If so, tell the ref registry, so old refs give `STALE_REF`.
/// Returns true when the document changed.
pub async fn sync_document(ctx: &Ctx, tab: TabId, state: &mut TabState) -> Result<bool, CmdError> {
    let loader = cdp::main_loader_id(ctx, tab).await?;
    let changed = match &state.loader_id {
        Some(old) => *old != loader,
        None => false,
    };
    if changed {
        state.registry.new_document();
    }
    state.loader_id = Some(loader);
    Ok(changed)
}

/// Fetch and parse the accessibility tree of the main frame.
pub async fn ax_tree(ctx: &Ctx, tab: TabId) -> Result<AxTree, CmdError> {
    let raw = cdp::call(ctx, tab, "Accessibility.getFullAXTree", json!({})).await?;
    AxTree::from_cdp(&raw).map_err(perception_error)
}

/// The snapshot header line: tab, title, url, viewport size and scroll position.
pub async fn header(ctx: &Ctx, tab: TabId) -> Result<PageHeader, CmdError> {
    let info = tab_info(ctx, tab).await?;
    // Layout metrics are nice to have; a failure must not break the snapshot.
    let layout = cdp::layout(ctx, tab).await.ok();
    Ok(PageHeader {
        tab: u32::try_from(info.id.0).unwrap_or_default(),
        title: info.title,
        url: info.url,
        viewport: layout.map(|l| Viewport {
            width: l.viewport_width.round() as u32,
            height: l.viewport_height.round() as u32,
        }),
        scroll: layout.map(|l| Scroll {
            y: l.page_y.max(0.0).round() as u32,
            height: l.content_height.round() as u32,
        }),
    })
}

/// Take a snapshot with `options`, remember it in the tab's history and return it.
pub async fn take_snapshot(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    options: SnapshotOptions,
) -> Result<Snapshot, CmdError> {
    Ok(take_snapshot_with_tree(ctx, tab, state, options).await?.0)
}

/// [`take_snapshot`] that also returns the accessibility tree it was built from.
pub async fn take_snapshot_with_tree(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    options: SnapshotOptions,
) -> Result<(Snapshot, AxTree), CmdError> {
    sync_document(ctx, tab, state).await?;
    let tree = ax_tree(ctx, tab).await?;
    let header = header(ctx, tab).await?;
    let snap = snapshot(&tree, &mut state.registry, &options, header).map_err(|err| {
        // Only a bad drill-down root (`root=`) makes a snapshot fail with a ref error.
        match &options.root {
            Some(root) => ref_error(ctx, tab, root, err),
            None => perception_error(err),
        }
    })?;
    state.remember(snap.clone(), options);
    Ok((snap, tree))
}

/// Resolve an element ref (`@4:e3`, `e3`) of `tab` to its DOM node. If the ref came from
/// another tab the error says which one.
pub fn resolve_ref(ctx: &Ctx, tab: TabId, state: &TabState, input: &str) -> Result<i64, CmdError> {
    state
        .registry
        .resolve(input)
        .map_err(|err| ref_error(ctx, tab, input, err))
}

/// The command error for a ref that failed in `tab`. Snapshot ids and e-numbers are unique
/// across tabs, so a ref from another tab never resolves here; when another tab handed it
/// out, tell the agent to pass that tab instead of a plain "no such ref".
pub fn ref_error(ctx: &Ctx, tab: TabId, input: &str, err: PerceptionError) -> CmdError {
    let is_ref_error = matches!(
        err,
        PerceptionError::BadRef { .. } | PerceptionError::StaleRef { .. }
    );
    let owner = if is_ref_error {
        PerceptionState::from_ctx(ctx).owner_of(tab, input)
    } else {
        None
    };
    match owner {
        Some(owner) => CmdError::new(
            ErrorCode::NotFound,
            format!(
                "ref {} belongs to tab {owner}, not to tab {tab}",
                input.trim()
            ),
        )
        .with_retry(Retry::No)
        .with_hint(format!(
            "Pass tab={owner}: a ref only works in the tab whose page_snapshot or page_find \
             printed it."
        )),
        None => perception_error(err),
    }
}

/// Options for the snapshots `page.act` compares to build its diff: everything (text too),
/// no budget and no list collapsing, so the diff sees every change. Their text is never shown.
pub fn diff_options() -> SnapshotOptions {
    SnapshotOptions {
        mode: Mode::Full,
        root: None,
        max_chars: usize::MAX,
        collapse_lists_over: usize::MAX,
        collapse_show: usize::MAX,
    }
}

/// After an action: give the page a moment, and if that started a navigation, wait until the
/// new page has loaded (at most [`SETTLE_MAX`]). Returns false if it was still loading then.
pub async fn settle(ctx: &Ctx, tab: TabId) -> Result<bool, CmdError> {
    tokio::time::sleep(SETTLE_FIRST_PAUSE).await;
    let started = tokio::time::Instant::now();
    let mut saw_loading = false;
    loop {
        let loading = tab_info(ctx, tab).await?.loading;
        saw_loading |= loading;
        let waited = started.elapsed();
        if !loading && (saw_loading || waited >= SETTLE_NAV_GRACE) {
            if saw_loading {
                // The load event fired; let the new page build its accessibility tree.
                tokio::time::sleep(POLL).await;
            }
            return Ok(true);
        }
        if waited >= SETTLE_MAX {
            return Ok(false);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until the tab is not loading (used before snapshots and by `nav.wait for=load`).
pub async fn wait_loaded(ctx: &Ctx, tab: TabId, timeout: Duration) -> Result<(), CmdError> {
    let started = tokio::time::Instant::now();
    loop {
        if !tab_info(ctx, tab).await?.loading {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(CmdError::new(
                ErrorCode::Timeout,
                format!("tab {tab} is still loading after {}s", timeout.as_secs()),
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}
