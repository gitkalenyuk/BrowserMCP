//! The commands: `page.snapshot`, `page.find`, `page.act`, `page.act_batch`, `page.diff`,
//! `page.read`, `page.screenshot`, `nav.back`, `nav.forward`, `nav.reload`, `nav.wait`,
//! `input.keys`, `input.humanlike`.
//!
//! Every command that reads or changes refs locks the tab's [`TabState`] for its whole run, so
//! two calls on one tab never interleave.

use std::time::Duration;

use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, TabId, TabInfo, Toolset};
use bmcp_perception::{Filter, Mode, SnapshotOptions, diff, find};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::act::{self, Action, Step};
use crate::cdp::{self, perception_error};
use crate::frames;
use crate::human::{self, Human, HumanProfile};
use crate::keys;
use crate::page;
use crate::state::{PerceptionState, TabState};

/// Default for `max_chars` of `page.read`.
const READ_DEFAULT_CHARS: usize = 4000;
/// Before a snapshot we wait this long for a loading page.
const SNAPSHOT_LOAD_WAIT: Duration = Duration::from_secs(5);

/// Lock the tab's perception state (see the module docs).
async fn lock_tab(ctx: &Ctx, tab: TabId) -> tokio::sync::OwnedMutexGuard<TabState> {
    let perception = PerceptionState::from_ctx(ctx);
    // Forget the state of tabs that were closed (or crashed) since the last call, so their
    // refs and snapshot history do not pile up. Not fatal when the list is unavailable.
    if let Ok(open) = ctx.backend.list_tabs().await {
        let open: Vec<TabId> = open.iter().map(|t| t.id).collect();
        perception.retain(&open);
    }
    perception.tab(tab).lock_owned().await
}

// ------------------------------------------------------------------ page.snapshot

pub struct Snapshot;

/// How much of the page a snapshot shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnapMode {
    /// Controls with refs, plus headings, alerts, dialogs and the landmarks around them.
    #[default]
    Interactive,
    /// Also text and images.
    Full,
    /// Only the outline (`depth` levels); deeper parts become `→ @S:gN` to drill into.
    Skeleton,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// interactive (default), full or skeleton.
    pub mode: Option<SnapMode>,
    /// Levels shown in skeleton mode (default 2).
    pub depth: Option<usize>,
    /// Drill down: show only this part, e.g. "@4:g1" (a collapsed group) or "@4:e3".
    pub root: Option<String>,
    /// Character budget (default 6000 ≈ 1500 tokens); over it the snapshot becomes a skeleton.
    pub max_chars: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SnapshotOutput {
    pub tab: TabId,
    /// Snapshot id: the S in refs `@S:eN`.
    pub snap: u64,
    /// Compact page text: one `- role "name" @S:eN states` line per node.
    pub text: String,
}

impl Command for Snapshot {
    const NAME: &'static str = "page.snapshot";
    type Args = SnapshotArgs;
    type Output = SnapshotOutput;

    fn meta() -> Meta {
        Meta::new(
            "Compact text view of the page from the accessibility tree: one line per control \
             with a ref (@S:eN) to act on, plus headings/landmarks. Use this instead of \
             screenshots; after actions use the diff they return.",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: SnapshotArgs) -> Result<SnapshotOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let mut options = SnapshotOptions {
            mode: match args.mode.unwrap_or_default() {
                SnapMode::Interactive => Mode::Interactive,
                SnapMode::Full => Mode::Full,
                SnapMode::Skeleton => Mode::Skeleton {
                    depth: args.depth.unwrap_or(2).max(1),
                },
            },
            root: args.root,
            ..SnapshotOptions::default()
        };
        if let Some(max) = args.max_chars {
            if max < 200 {
                return Err(CmdError::invalid_params("max_chars must be at least 200"));
            }
            options.max_chars = max;
        }
        // A page that is still loading gives a half-built tree; wait a little (not fatal).
        let _ = page::wait_loaded(&ctx, tab, SNAPSHOT_LOAD_WAIT).await;
        let mut state = lock_tab(&ctx, tab).await;
        let snap = page::take_snapshot(&ctx, tab, &mut state, options).await?;
        Ok(SnapshotOutput {
            tab,
            snap: snap.id,
            text: snap.text,
        })
    }
}

// ------------------------------------------------------------------ page.find

pub struct Find;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Role such as button, link, textbox, checkbox, heading.
    pub role: Option<String>,
    /// Exact accessible name (case-insensitive).
    pub name: Option<String>,
    /// Case-insensitive substring of name, description or value; also finds plain text.
    pub text: Option<String>,
    /// Regular expression on the name, e.g. "(?i)^sign (in|up)$".
    pub regex: Option<String>,
    /// Max matches returned (default 10).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FindOutput {
    pub tab: TabId,
    /// Total matches before `limit`.
    pub total: usize,
    /// `find S: N of M matches` and one line per match with its ref and location.
    pub text: String,
}

impl Command for Find {
    const NAME: &'static str = "page.find";
    type Args = FindArgs;
    type Output = FindOutput;

    fn meta() -> Meta {
        Meta::new(
            "Search the whole page (also parts a snapshot collapsed) by role, name, text or \
             regex. Returns matches with refs to use in page_act. Cheaper than a snapshot \
             when you know what you need.",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: FindArgs) -> Result<FindOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let filter = Filter {
            role: args.role,
            name: args.name,
            text: args.text,
            regex: args.regex,
        };
        let mut state = lock_tab(&ctx, tab).await;
        page::sync_document(&ctx, tab, &mut state).await?;
        let tree = page::ax_tree(&ctx, tab, &mut state).await?;
        let found = find(
            &tree,
            &mut state.registry,
            &filter,
            args.limit.unwrap_or(10).max(1),
        )
        .map_err(perception_error)?;
        Ok(FindOutput {
            tab,
            total: found.total,
            text: found.to_text(),
        })
    }
}

// ------------------------------------------------------------------ page.act

pub struct Act;

/// What an action returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Returns {
    /// One line with what changed on the page (default).
    #[default]
    Diff,
    /// A whole new interactive snapshot.
    Snapshot,
    /// Nothing but the confirmation line.
    None,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Element ref from page_snapshot / page_find, e.g. "@4:e3". Optional only for press
    /// (keys go to the focused element) and scroll (scrolls the page).
    #[serde(rename = "ref")]
    pub target: Option<String>,
    /// click, dblclick, fill, type, select, check, uncheck, hover, press, scroll, focus.
    pub action: Action,
    /// Text for fill/type, option value or label for select, keys for press ("Enter",
    /// "Control+A", "Tab Tab"), direction for scroll (up/down/top/bottom/into_view).
    pub value: Option<String>,
    /// diff (default), snapshot or none.
    pub returns: Option<Returns>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ActOutput {
    pub tab: TabId,
    /// True when the action loaded a new page (then `text` holds a full new snapshot and all
    /// old refs are stale).
    pub navigated: bool,
    /// What was done, then the diff (`diff 4→5: ...`) or snapshot.
    pub text: String,
}

impl Command for Act {
    const NAME: &'static str = "page.act";
    type Args = ActArgs;
    type Output = ActOutput;

    fn meta() -> Meta {
        Meta::new(
            "Act on an element by ref: click, dblclick, fill, type, select, check, uncheck, \
             hover, press, scroll, focus. Checks the element is visible, enabled and not \
             covered (else NOT_INTERACTABLE naming what is in the way), uses real input \
             events, waits for the page, and returns a one-line diff of what changed.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &ActArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: ActArgs) -> Result<ActOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let step = Step {
            target: args.target,
            action: args.action,
            value: args.value,
        };
        let mut state = lock_tab(&ctx, tab).await;
        let returns = args.returns.unwrap_or_default();
        let run = run_steps(&ctx, tab, &mut state, &[step], true, returns).await?;
        let result = run.results.into_iter().next().expect("one step was run");
        match result.error {
            Some(err) => Err(err),
            None => Ok(ActOutput {
                tab,
                navigated: run.navigated,
                text: format!("{}\n{}", result.text, run.text)
                    .trim_end()
                    .to_string(),
            }),
        }
    }
}

/// One step's result inside [`run_steps`].
struct StepRun {
    text: String,
    error: Option<CmdError>,
}

struct RunResult {
    results: Vec<StepRun>,
    navigated: bool,
    /// The diff / snapshot / notes after the last step.
    text: String,
}

/// Shared by `page.act` and `page.act_batch`: baseline snapshot, steps, wait, diff.
async fn run_steps(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    steps: &[Step],
    stop_on_error: bool,
    returns: Returns,
) -> Result<RunResult, CmdError> {
    // A new document since the agent's snapshot makes every old ref stale: find out first.
    page::sync_document(ctx, tab, state).await?;
    let document = state.registry.document();
    // Resolve the first ref now, so a bad ref fails before we spend time on a snapshot.
    if let Some(first) = steps.first().and_then(|s| s.target.as_deref()) {
        page::resolve_ref(ctx, tab, state, first)?;
    }
    // "Before" picture for the diff, with everything in it (text too) so no change is missed.
    let before = if returns == Returns::Diff {
        Some(page::take_snapshot(ctx, tab, state, page::diff_options()).await?)
    } else {
        None
    };

    let mut results = Vec::new();
    let mut settled = true;
    let count = steps.len();
    for (index, step) in steps.iter().enumerate() {
        match act::perform(ctx, tab, state, step).await {
            Ok(text) => results.push(StepRun { text, error: None }),
            Err(err) => {
                let text = format!("{}: {}", err.code, err.message);
                results.push(StepRun {
                    text,
                    error: Some(err),
                });
                if stop_on_error {
                    break;
                }
            }
        }
        // Wait for the page after each step that may navigate, and after the last one.
        if step.action.may_navigate() || index + 1 == count {
            settled = page::settle(ctx, tab).await?;
            page::sync_document(ctx, tab, state).await?;
        }
    }

    let navigated = state.registry.document() != document;
    let mut text = String::new();
    if !settled {
        text.push_str("! the page was still loading after 10s\n");
    }
    if navigated && returns != Returns::None {
        // A new page: the diff would list everything; show a fresh snapshot instead.
        let snap = page::take_snapshot(ctx, tab, state, SnapshotOptions::default()).await?;
        text.push_str(&format!("navigated to {}\n{}", snap.header.url, snap.text));
    } else {
        match (returns, before) {
            (Returns::Diff, Some(before)) => {
                let after = page::take_snapshot(ctx, tab, state, page::diff_options()).await?;
                text.push_str(&diff(&before, &after));
            }
            (Returns::Snapshot, _) => {
                let snap = page::take_snapshot(ctx, tab, state, SnapshotOptions::default()).await?;
                text.push_str(&snap.text);
            }
            _ => {}
        }
    }
    Ok(RunResult {
        results,
        navigated,
        text,
    })
}

// ------------------------------------------------------------------ page.act_batch

pub struct ActBatch;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActBatchArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Steps in order, each {ref, action, value?} like page_act.
    pub steps: Vec<Step>,
    /// Stop at the first failing step (default true).
    pub stop_on_error: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BatchStepResult {
    /// 1-based step number.
    pub step: usize,
    pub ok: bool,
    /// What was done, or `CODE: message`.
    pub text: String,
    /// The full error ({code, message, retry, hint}) when the step failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ActBatchOutput {
    pub tab: TabId,
    /// True when every step ran and succeeded.
    pub ok: bool,
    pub steps: Vec<BatchStepResult>,
    pub navigated: bool,
    /// Diff (or snapshot after a navigation) after the last step.
    pub text: String,
}

impl Command for ActBatch {
    const NAME: &'static str = "page.act_batch";
    type Args = ActBatchArgs;
    type Output = ActBatchOutput;

    fn meta() -> Meta {
        Meta::new(
            "Run several page_act steps in one call (e.g. fill email, fill password, click \
             submit). Returns a result per step and one diff at the end.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &ActBatchArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: ActBatchArgs) -> Result<ActBatchOutput, CmdError> {
        if args.steps.is_empty() {
            return Err(CmdError::invalid_params("steps is empty"));
        }
        if args.steps.len() > 50 {
            return Err(CmdError::invalid_params("at most 50 steps per batch"));
        }
        let tab = ctx.resolve_tab(args.tab).await?;
        let mut state = lock_tab(&ctx, tab).await;
        let stop = args.stop_on_error.unwrap_or(true);
        let run = run_steps(&ctx, tab, &mut state, &args.steps, stop, Returns::Diff).await?;
        let steps: Vec<BatchStepResult> = run
            .results
            .into_iter()
            .enumerate()
            .map(|(i, r)| BatchStepResult {
                step: i + 1,
                ok: r.error.is_none(),
                text: r.text,
                error: r.error.map(|e| serde_json::to_value(e).unwrap_or_default()),
            })
            .collect();
        Ok(ActBatchOutput {
            tab,
            ok: steps.len() == args.steps.len() && steps.iter().all(|s| s.ok),
            steps,
            navigated: run.navigated,
            text: run.text,
        })
    }
}

// ------------------------------------------------------------------ page.diff

pub struct Diff;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Snapshot id to compare with (the S of @S:eN). Default: the latest one.
    pub since: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiffOutput {
    pub tab: TabId,
    /// Id of the new snapshot taken for the comparison.
    pub snap: u64,
    /// `diff A→B: + added | ~ changed | - removed`, or a full snapshot if the page navigated.
    pub text: String,
}

impl Command for Diff {
    const NAME: &'static str = "page.diff";
    type Args = DiffArgs;
    type Output = DiffOutput;

    fn meta() -> Meta {
        Meta::new(
            "What changed on the page since a snapshot (default: the latest), in one line. \
             Use it to watch for results that appear later (messages, loaded lists).",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: DiffArgs) -> Result<DiffOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let mut state = lock_tab(&ctx, tab).await;
        let base = match args.since {
            Some(id) => state.by_id(id).cloned().ok_or_else(|| {
                CmdError::new(
                    ErrorCode::NotFound,
                    format!("snapshot {id} is unknown or too old"),
                )
                .with_hint("Omit since to compare with the latest snapshot.")
            })?,
            None => state.history.back().cloned().ok_or_else(|| {
                CmdError::new(ErrorCode::NotFound, "no snapshot of this tab yet")
                    .with_hint("Take a page_snapshot first.")
            })?,
        };
        let (before, options) = base;
        let navigated = page::sync_document(&ctx, tab, &mut state).await?
            || before.document != state.registry.document();
        if navigated {
            let snap =
                page::take_snapshot(&ctx, tab, &mut state, SnapshotOptions::default()).await?;
            return Ok(DiffOutput {
                tab,
                snap: snap.id,
                text: format!("navigated to {}\n{}", snap.header.url, snap.text),
            });
        }
        let after = page::take_snapshot(&ctx, tab, &mut state, options).await?;
        Ok(DiffOutput {
            tab,
            snap: after.id,
            text: diff(&before, &after),
        })
    }
}

// ------------------------------------------------------------------ page.read

pub struct Read;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Read only this element (e.g. a dialog or article) instead of the whole page.
    #[serde(rename = "ref")]
    pub target: Option<String>,
    /// Maximum characters (default 4000).
    pub max_chars: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReadOutput {
    pub tab: TabId,
    /// Visible text (innerText).
    pub text: String,
    pub truncated: bool,
    pub total_chars: usize,
}

impl Command for Read {
    const NAME: &'static str = "page.read";
    type Args = ReadArgs;
    type Output = ReadOutput;

    fn meta() -> Meta {
        Meta::new(
            "Visible text of the page or of one element (ref), cut to max_chars. Use it to \
             read articles, messages or table contents a snapshot only names.",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: ReadArgs) -> Result<ReadOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let limit = args.max_chars.unwrap_or(READ_DEFAULT_CHARS).max(1);
        let value = match &args.target {
            Some(target) => {
                let mut state = lock_tab(&ctx, tab).await;
                page::sync_document(&ctx, tab, &mut state).await?;
                let node = page::resolve_node(&ctx, tab, &state, target)?;
                let session = node.session.as_deref();
                let object = cdp::resolve_object(&ctx, tab, session, node.backend, target).await?;
                cdp::call_function(&ctx, tab, session, &object, act::READ_JS, &[]).await?
            }
            None => {
                // The same text rules as for one element (shadow DOM included).
                let expression = format!(
                    "document.body ? ({}).call(document.body) : ''",
                    act::READ_JS
                );
                cdp::evaluate(&ctx, tab, &expression).await?
            }
        };
        let full = value.as_str().unwrap_or_default();
        let total_chars = full.chars().count();
        Ok(ReadOutput {
            tab,
            text: full.chars().take(limit).collect(),
            truncated: total_chars > limit,
            total_chars,
        })
    }
}

// ------------------------------------------------------------------ page.screenshot

pub struct Screenshot;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Crop to this element.
    #[serde(rename = "ref")]
    pub target: Option<String>,
    /// The whole page instead of the visible part.
    pub full_page: Option<bool>,
    /// Size factor, 0.1–2 (default 0.5: half size, few tokens).
    pub scale: Option<f64>,
    /// JPEG quality 1–100 (default 70).
    pub quality: Option<u8>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScreenshotOutput {
    pub tab: TabId,
    /// Always "image/jpeg".
    pub mime_type: String,
    /// Image size in pixels.
    pub width: u32,
    pub height: u32,
    /// Base64 JPEG.
    pub data: String,
}

impl Command for Screenshot {
    const NAME: &'static str = "page.screenshot";
    type Args = ScreenshotArgs;
    type Output = ScreenshotOutput;

    fn meta() -> Meta {
        Meta::new(
            "JPEG screenshot of the visible page, the full page or one element (ref). Costs \
             many tokens: use only when page_snapshot/page_read are not enough (canvas, \
             images, layout questions).",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: ScreenshotArgs) -> Result<ScreenshotOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let scale = args.scale.unwrap_or(0.5);
        if !(0.1..=2.0).contains(&scale) {
            return Err(CmdError::invalid_params("scale must be between 0.1 and 2"));
        }
        let quality = args.quality.unwrap_or(70).clamp(1, 100);
        let layout = cdp::layout(&ctx, tab, None).await?;

        // The clip is in CSS pixels of the document (not the viewport), so add the scroll.
        let (x, y, width, height) = if let Some(target) = &args.target {
            let mut state = lock_tab(&ctx, tab).await;
            page::sync_document(&ctx, tab, &mut state).await?;
            let node = page::resolve_node(&ctx, tab, &state, target)?;
            let session = node.session.as_deref();
            let _ = cdp::call_in(
                &ctx,
                tab,
                session,
                "DOM.scrollIntoViewIfNeeded",
                json!({ "backendNodeId": node.backend }),
            )
            .await;
            let rect = cdp::content_box(&ctx, tab, session, node.backend)
                .await?
                .ok_or_else(|| {
                    act::not_interactable(
                        format!("{target} is not visible (no box on the page)"),
                        "Take a page_snapshot; it may be hidden or collapsed.",
                    )
                })?;
            // Inside an out-of-process iframe the box is in the frame's viewport.
            let (dx, dy) = frames::frame_offset(&ctx, tab, &state.frames, node.slot).await?;
            // Scrolling into view changed the scroll offset: read it again.
            let layout = cdp::layout(&ctx, tab, None).await?;
            (
                rect.x + dx + layout.page_x,
                rect.y + dy + layout.page_y,
                rect.width,
                rect.height,
            )
        } else if args.full_page.unwrap_or(false) {
            (0.0, 0.0, layout.content_width, layout.content_height)
        } else {
            (
                layout.page_x,
                layout.page_y,
                layout.viewport_width,
                layout.viewport_height,
            )
        };
        let reply = cdp::call(
            &ctx,
            tab,
            "Page.captureScreenshot",
            json!({
                "format": "jpeg",
                "quality": quality,
                "clip": { "x": x, "y": y, "width": width, "height": height, "scale": scale },
                "captureBeyondViewport": args.full_page.unwrap_or(false),
                "fromSurface": true,
            }),
        )
        .await?;
        let data = reply["data"]
            .as_str()
            .ok_or_else(|| CmdError::internal("the browser returned no image"))?;
        Ok(ScreenshotOutput {
            tab,
            mime_type: "image/jpeg".to_string(),
            width: (width * scale).round() as u32,
            height: (height * scale).round() as u32,
            data: data.to_string(),
        })
    }
}

// ------------------------------------------------------------------ nav.back / forward / reload

/// Shared arguments of `nav.back`, `nav.forward`, `nav.reload`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Max seconds to wait for the page to load (default 30).
    pub timeout_secs: Option<u64>,
}

fn nav_timeout(timeout_secs: Option<u64>) -> Result<Duration, CmdError> {
    let secs = timeout_secs.unwrap_or(30);
    if secs == 0 || secs > 600 {
        return Err(CmdError::invalid_params(
            "timeout_secs must be between 1 and 600",
        ));
    }
    Ok(Duration::from_secs(secs))
}

/// Go `delta` (-1 back, +1 forward) in the tab's history and wait for the page.
async fn go_history(
    ctx: &Ctx,
    tab: TabId,
    delta: i64,
    timeout: Duration,
) -> Result<TabInfo, CmdError> {
    let history = cdp::call(ctx, tab, "Page.getNavigationHistory", json!({})).await?;
    let current = history["currentIndex"].as_i64().unwrap_or(0);
    let entries = history["entries"].as_array().cloned().unwrap_or_default();
    let wanted = current + delta;
    let entry = usize::try_from(wanted)
        .ok()
        .and_then(|i| entries.get(i))
        .ok_or_else(|| {
            let which = if delta < 0 { "previous" } else { "next" };
            CmdError::new(
                ErrorCode::NotFound,
                format!("tab {tab} has no {which} page in its history"),
            )
            .with_retry(bmcp_core::Retry::No)
        })?;
    cdp::call(
        ctx,
        tab,
        "Page.navigateToHistoryEntry",
        json!({ "entryId": entry["id"] }),
    )
    .await?;
    page::settle(ctx, tab).await?;
    page::wait_loaded(ctx, tab, timeout).await?;
    page::tab_info(ctx, tab).await
}

pub struct Back;

impl Command for Back {
    const NAME: &'static str = "nav.back";
    type Args = HistoryArgs;
    type Output = TabInfo;

    fn meta() -> Meta {
        Meta::new(
            "Go back one page in the tab's history and wait for it to load. Old refs become \
             stale: take a new page_snapshot.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
        .palette("Back")
    }

    fn target_tab(args: &HistoryArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: HistoryArgs) -> Result<TabInfo, CmdError> {
        let timeout = nav_timeout(args.timeout_secs)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        go_history(&ctx, tab, -1, timeout).await
    }
}

pub struct Forward;

impl Command for Forward {
    const NAME: &'static str = "nav.forward";
    type Args = HistoryArgs;
    type Output = TabInfo;

    fn meta() -> Meta {
        Meta::new(
            "Go forward one page in the tab's history and wait for it to load.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
        .palette("Forward")
    }

    fn target_tab(args: &HistoryArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: HistoryArgs) -> Result<TabInfo, CmdError> {
        let timeout = nav_timeout(args.timeout_secs)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        go_history(&ctx, tab, 1, timeout).await
    }
}

pub struct Reload;

impl Command for Reload {
    const NAME: &'static str = "nav.reload";
    type Args = HistoryArgs;
    type Output = TabInfo;

    fn meta() -> Meta {
        Meta::new(
            "Reload the page and wait for it to load. Old refs become stale.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
        .palette("Reload")
    }

    fn target_tab(args: &HistoryArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: HistoryArgs) -> Result<TabInfo, CmdError> {
        let timeout = nav_timeout(args.timeout_secs)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        cdp::call(&ctx, tab, "Page.reload", json!({})).await?;
        page::settle(&ctx, tab).await?;
        page::wait_loaded(&ctx, tab, timeout).await?;
        page::tab_info(&ctx, tab).await
    }
}

// ------------------------------------------------------------------ nav.wait

pub struct Wait;

/// What `nav.wait` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitFor {
    /// The page finished loading.
    Load,
    /// `value` appears in the page text (case-insensitive).
    Text,
    /// The element `value` (a ref) is visible.
    Ref,
    /// The URL contains `value`.
    Url,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// load, text, ref or url.
    #[serde(rename = "for")]
    pub condition: WaitFor,
    /// The text, ref or URL part to wait for (not needed for load).
    pub value: Option<String>,
    /// Max seconds to wait (default 10, max 120).
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WaitOutput {
    pub tab: TabId,
    /// How long it took.
    pub waited_ms: u64,
}

impl Command for Wait {
    const NAME: &'static str = "nav.wait";
    type Args = WaitArgs;
    type Output = WaitOutput;

    fn meta() -> Meta {
        Meta::new(
            "Wait until the page has loaded, a text appears, an element (ref) is visible, or \
             the URL contains a value. TIMEOUT if it does not happen in time.",
            Toolset::Core,
        )
    }

    async fn run(ctx: Ctx, args: WaitArgs) -> Result<WaitOutput, CmdError> {
        let secs = args.timeout_secs.unwrap_or(10);
        if secs == 0 || secs > 120 {
            return Err(CmdError::invalid_params(
                "timeout_secs must be between 1 and 120",
            ));
        }
        let timeout = Duration::from_secs(secs);
        let tab = ctx.resolve_tab(args.tab).await?;
        let value = match (args.condition, args.value) {
            (WaitFor::Load, _) => String::new(),
            (_, Some(value)) if !value.trim().is_empty() => value,
            (condition, _) => {
                return Err(CmdError::invalid_params(format!(
                    "for={condition:?} needs a value"
                )));
            }
        };
        let started = tokio::time::Instant::now();
        loop {
            if wait_condition_met(&ctx, tab, args.condition, &value).await? {
                return Ok(WaitOutput {
                    tab,
                    waited_ms: started.elapsed().as_millis() as u64,
                });
            }
            if started.elapsed() >= timeout {
                return Err(CmdError::new(
                    ErrorCode::Timeout,
                    format!(
                        "waited {secs}s for {:?} {value:?} in tab {tab}",
                        args.condition
                    ),
                )
                .with_hint("Take a page_snapshot to see what the page shows instead."));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

async fn wait_condition_met(
    ctx: &Ctx,
    tab: TabId,
    condition: WaitFor,
    value: &str,
) -> Result<bool, CmdError> {
    Ok(match condition {
        WaitFor::Load => !page::tab_info(ctx, tab).await?.loading,
        WaitFor::Url => page::tab_info(ctx, tab).await?.url.contains(value),
        WaitFor::Text => {
            // The value goes in as a JSON string literal, so quotes in it cannot break out.
            let needle = serde_json::to_string(&value.to_lowercase()).unwrap_or_default();
            let expression = format!(
                "(document.body ? document.body.innerText : '').toLowerCase().includes({needle})"
            );
            // Pages being replaced can fail an evaluation; just try again next round.
            cdp::evaluate(ctx, tab, &expression)
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        }
        WaitFor::Ref => {
            let mut state = lock_tab(ctx, tab).await;
            page::sync_document(ctx, tab, &mut state).await?;
            let node = page::resolve_node(ctx, tab, &state, value)?;
            cdp::content_box(ctx, tab, node.session.as_deref(), node.backend)
                .await?
                .is_some()
        }
    })
}

// ------------------------------------------------------------------ input.keys

pub struct Keys;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeysArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Chords separated by spaces: "Enter", "Control+A", "Shift+Tab", "ArrowDown ArrowDown
    /// Enter".
    pub keys: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct KeysOutput {
    pub tab: TabId,
    /// Number of chords pressed.
    pub pressed: usize,
}

impl Command for Keys {
    const NAME: &'static str = "input.keys";
    type Args = KeysArgs;
    type Output = KeysOutput;

    fn meta() -> Meta {
        Meta::new(
            "Press keys or shortcuts on the focused element (\"Enter\", \"Control+A\", \
             \"Tab Tab Enter\"). To type text into a field use page_act fill/type.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &KeysArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: KeysArgs) -> Result<KeysOutput, CmdError> {
        let chords = keys::parse_keys(&args.keys)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let _state = lock_tab(&ctx, tab).await;
        act::ensure_page_visible(&ctx, tab).await?;
        keys::press(&ctx, tab, &chords).await?;
        page::settle(&ctx, tab).await?;
        Ok(KeysOutput {
            tab,
            pressed: chords.len(),
        })
    }
}

// ------------------------------------------------------------------ input.humanlike

pub struct Humanlike;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanlikeArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// true = human-like input for page_act in this tab; false = the fast path (default).
    pub on: bool,
    /// Speeds and irregularity: {mouse_speed (px/s, 1200), typing_wpm (55), jitter (0-1,
    /// 0.35)}. Missing fields keep their defaults.
    pub profile: Option<HumanProfile>,
    /// Seed for the random paths and delays (for reproducible runs). Default: random.
    pub seed: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HumanlikeOutput {
    pub tab: TabId,
    pub on: bool,
    /// The profile in use (after clamping to sane ranges).
    pub profile: Option<HumanProfile>,
}

impl Command for Humanlike {
    const NAME: &'static str = "input.humanlike";
    type Args = HumanlikeArgs;
    type Output = HumanlikeOutput;

    fn meta() -> Meta {
        Meta::new(
            "Switch a tab between fast input (default) and human-like input: page_act then \
             moves the mouse on curved paths with easing and slight overshoot, types \
             character by character with natural delays and scrolls with wheel notches. \
             Slower, but looks like a person.",
            Toolset::Stealth,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &HumanlikeArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: HumanlikeArgs) -> Result<HumanlikeOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let mut state = lock_tab(&ctx, tab).await;
        state.human = args.on.then(|| {
            let seed = args
                .seed
                .unwrap_or_else(|| human::Rng::from_time().next_u64());
            Human::new(args.profile.unwrap_or_default(), seed)
        });
        Ok(HumanlikeOutput {
            tab,
            on: args.on,
            profile: state.human.as_ref().map(|h| h.profile),
        })
    }
}
