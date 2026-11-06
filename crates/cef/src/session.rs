//! Session restore: every composed window's tabs are written to
//! `data/runtime/session.json` (debounced, 1 s after the last tab change) and opened again on
//! the next start (`--no-restore` skips that). Plus the closed-tabs stack behind
//! `tabs.restore_closed` / Ctrl+Shift+T (kept in the same file).
//!
//! What is saved per window: each tab's URL, title and profile, the active tab and the split
//! view. Never saved: incognito tabs, off-screen tabs (they have no window), windows that are
//! closing. A snapshot without any window is never written, so quitting (every window is
//! closing) or closing the last tab keeps the session of the moment before.
//!
//! Restoring: tabs of the launch profile open at once. Tabs of other profiles wait until the
//! app has applied the stored profile settings ([`crate::profiles_applied`]): their proxy
//! (and identity) must be in place before they load anything. Until then nothing is saved,
//! so a failed start does not lose them.
//!
//! The model and the decisions are pure ([`snapshot`], [`plan_window`]) and unit-tested; the
//! rest runs on the CEF UI thread.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use bmcp_core::{BackendError, TabId};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::bridge::{PendingTab, post_to_ui, post_to_ui_delayed, with_ui};
use crate::profiles::ProfileKey;

// ------------------------------------------------------------------ the file (pure)

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub windows: Vec<SessionWindow>,
    /// Recently closed tabs, most recent last.
    #[serde(default)]
    pub closed: Vec<ClosedTab>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionWindow {
    pub tabs: Vec<SessionTab>,
    /// Index of the shown tab.
    #[serde(default)]
    pub active: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<SessionSplit>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// `None` = the profile the browser started with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Split view of a window: tab indexes and the left half's share.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionSplit {
    pub left: usize,
    pub right: usize,
    pub ratio: f64,
}

/// One entry of the closed-tabs stack.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ClosedTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Position in its window's tab strip.
    #[serde(default)]
    pub index: usize,
    /// Its window (this run only).
    #[serde(skip)]
    pub window: Option<i32>,
}

/// How many closed tabs are remembered.
pub(crate) const CLOSED_MAX: usize = 25;

/// A tab as [`snapshot`] sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LiveTab {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub profile: Option<String>,
    pub incognito: bool,
}

/// A composed window as [`snapshot`] sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LiveWindow {
    pub tabs: Vec<LiveTab>,
    pub active: Option<TabId>,
    pub split: Option<(TabId, TabId, f64)>,
    pub closing: bool,
}

/// May this tab be saved? Not in memory only, and showing something real.
fn persistable(tab: &LiveTab) -> bool {
    !tab.incognito && !tab.url.is_empty() && tab.url != "about:blank"
}

/// The session file for these windows, or `None` when nothing would be left to restore
/// (then the file is not touched). Pure.
pub(crate) fn snapshot(windows: &[LiveWindow], closed: &[ClosedTab]) -> Option<SessionFile> {
    let mut out = Vec::new();
    for w in windows.iter().filter(|w| !w.closing) {
        let kept: Vec<&LiveTab> = w.tabs.iter().filter(|t| persistable(t)).collect();
        if kept.is_empty() {
            continue;
        }
        let index_of = |id: TabId| kept.iter().position(|t| t.id == id);
        let active = w.active.and_then(index_of).unwrap_or(0);
        let split = w.split.and_then(|(l, r, ratio)| {
            Some(SessionSplit {
                left: index_of(l)?,
                right: index_of(r)?,
                ratio,
            })
        });
        out.push(SessionWindow {
            tabs: kept
                .iter()
                .map(|t| SessionTab {
                    url: t.url.clone(),
                    title: t.title.clone(),
                    profile: t.profile.clone(),
                })
                .collect(),
            active,
            split,
        });
    }
    (!out.is_empty()).then(|| SessionFile {
        version: 1,
        windows: out,
        closed: closed.to_vec(),
    })
}

/// One tab to restore.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedTab {
    /// Position in the window.
    pub index: usize,
    pub url: String,
    pub profile: Option<String>,
    /// The window's shown tab.
    pub activate: bool,
    /// Waits for the profile settings (any profile but the launch one).
    pub deferred: bool,
}

/// The tabs of one saved window, in order. Pure.
pub(crate) fn plan_window(window: &SessionWindow, launch_profile: &str) -> Vec<PlannedTab> {
    let active = window.active.min(window.tabs.len().saturating_sub(1));
    window
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let profile = tab.profile.clone().filter(|p| p != launch_profile);
            PlannedTab {
                index,
                url: tab.url.clone(),
                deferred: profile.is_some(),
                profile,
                activate: index == active,
            }
        })
        .collect()
}

/// Where a restored tab goes in its window's strip: after every tab of the same window
/// with a smaller saved index that exists already (tabs register in any order). Pure.
pub(crate) fn insert_index(present: &BTreeMap<usize, TabId>, index: usize) -> usize {
    present.range(..index).count()
}

/// Push onto the closed-tabs stack (oldest dropped beyond [`CLOSED_MAX`]). Pure.
pub(crate) fn push_closed(stack: &mut Vec<ClosedTab>, tab: ClosedTab) {
    stack.push(tab);
    if stack.len() > CLOSED_MAX {
        let extra = stack.len() - CLOSED_MAX;
        stack.drain(..extra);
    }
}

// ------------------------------------------------------------------ state (UI thread)

/// A window being restored, by restore tag.
#[derive(Debug, Default)]
pub(crate) struct RestoringWindow {
    /// Its window key once it exists.
    pub key: Option<i32>,
    /// Restored tabs so far, by saved index.
    pub ids: BTreeMap<usize, TabId>,
    pub split: Option<SessionSplit>,
}

/// Session state inside `UiState`.
#[derive(Default)]
pub(crate) struct SessionState {
    pub closed: Vec<ClosedTab>,
    pub restoring: BTreeMap<u32, RestoringWindow>,
    /// Tabs waiting for [`crate::profiles_applied`], with their restore tag.
    pub deferred: Vec<(u32, PlannedTab)>,
}

thread_local! {
    /// A save is scheduled (debounce).
    static SCHEDULED: Cell<bool> = const { Cell::new(false) };
    /// Deferred tabs are not restored yet: saving now would forget them.
    static HOLD: Cell<bool> = const { Cell::new(false) };
}

fn session_path() -> PathBuf {
    crate::profiles::launch()
        .data_root
        .join("runtime")
        .join("session.json")
}

/// Something about the tabs changed: save in a second. Any thread; never touches
/// `UiState` directly (it may be borrowed by the caller).
pub(crate) fn changed() {
    if cef::currently_on(cef::ThreadId::UI) == 0 {
        post_to_ui(Box::new(changed));
        return;
    }
    if SCHEDULED.get() || HOLD.get() {
        return;
    }
    SCHEDULED.set(true);
    post_to_ui_delayed(Box::new(save_now), 1000);
}

/// Write the session file now (UI thread).
fn save_now() {
    SCHEDULED.set(false);
    if HOLD.get() {
        return;
    }
    let (windows, closed) = with_ui(|ui| {
        let windows: Vec<LiveWindow> = ui
            .windows
            .values()
            .map(|w| LiveWindow {
                tabs: w
                    .strip
                    .tabs()
                    .iter()
                    .filter_map(|id| {
                        let t = ui.tabs.get(id)?;
                        Some(LiveTab {
                            id: *id,
                            url: t.url.clone(),
                            title: t.title.clone(),
                            profile: t.profile.id.clone(),
                            incognito: t.profile.incognito,
                        })
                    })
                    .collect(),
                active: w.strip.active(),
                split: w.split.map(|s| (s.left, s.right, s.ratio)),
                closing: w.closing,
            })
            .collect();
        (windows, ui.session.closed.clone())
    });
    let Some(file) = snapshot(&windows, &closed) else {
        return;
    };
    let path = session_path();
    if let Err(err) = write_atomic(&path, &file) {
        tracing::warn!(path = %path.display(), %err, "session not saved");
    }
}

/// Write via a temporary file + rename, so a crash never leaves half a file.
fn write_atomic(path: &std::path::Path, file: &SessionFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(file).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// The saved session, if there is a readable one.
pub(crate) fn load() -> Option<SessionFile> {
    let text = std::fs::read_to_string(session_path()).ok()?;
    match serde_json::from_str::<SessionFile>(&text) {
        Ok(file) => Some(file),
        Err(err) => {
            tracing::warn!(%err, "session.json unreadable; starting fresh");
            None
        }
    }
}

// ------------------------------------------------------------------ restoring

fn next_tag() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Open the windows of `file` (UI thread, at startup). Returns false when there was
/// nothing to open (the caller then opens the start page).
pub(crate) fn restore(file: SessionFile, ctx: &crate::bridge::ShellContext) -> bool {
    with_ui(|ui| ui.session.closed = file.closed.clone());
    let launch = crate::profiles::launch().profile.clone();
    let mut opened = false;
    for window in &file.windows {
        let tag = next_tag();
        let plan = plan_window(window, &launch);
        if plan.is_empty() {
            continue;
        }
        with_ui(|ui| {
            ui.session.restoring.insert(
                tag,
                RestoringWindow {
                    split: window.split,
                    ..Default::default()
                },
            )
        });
        let (now, later): (Vec<PlannedTab>, Vec<PlannedTab>) =
            plan.into_iter().partition(|t| !t.deferred);
        if !later.is_empty() {
            HOLD.set(true);
            with_ui(|ui| {
                ui.session
                    .deferred
                    .extend(later.into_iter().map(|t| (tag, t)))
            });
        }
        let mut now = now.into_iter();
        let Some(first) = now.next() else {
            continue; // the window opens with its first deferred tab
        };
        let key = crate::chrome::open_window_key(pending_for(tag, &first), ctx);
        opened = true;
        let Some(key) = key else { continue };
        with_ui(|ui| {
            if let Some(r) = ui.session.restoring.get_mut(&tag) {
                r.key = Some(key);
            }
        });
        for tab in now {
            crate::chrome::add_tab(ctx, key, pending_for(tag, &tab));
        }
    }
    tracing::info!(windows = file.windows.len(), "session restored");
    opened
}

/// The pending tab for a restored one.
fn pending_for(tag: u32, tab: &PlannedTab) -> PendingTab {
    let profile = ProfileKey::from_options(tab.profile.as_deref(), false).unwrap_or_default();
    let mut pending = PendingTab::new(&tab.url, profile, None);
    pending.activate = tab.activate;
    pending.restore = Some((tag, tab.index));
    pending
}

/// The app applied the stored profile settings: open the tabs that waited for them.
/// UI thread.
pub(crate) fn restore_deferred() {
    let Some(ctx) = with_ui(|ui| ui.shell.clone()) else {
        return;
    };
    let deferred = with_ui(|ui| std::mem::take(&mut ui.session.deferred));
    HOLD.set(false);
    if deferred.is_empty() {
        return;
    }
    tracing::info!(tabs = deferred.len(), "restoring tabs of other profiles");
    for (tag, tab) in deferred {
        let pending = pending_for(tag, &tab);
        let ctx = ctx.clone();
        let profile = pending.profile.clone();
        crate::profiles::when_ready(
            &profile,
            Box::new(move || {
                let key = with_ui(|ui| {
                    let key = ui.session.restoring.get(&tag)?.key?;
                    ui.windows.contains_key(&key).then_some(key)
                });
                match key {
                    Some(key) => crate::chrome::add_tab(&ctx, key, pending),
                    None => {
                        // The window had no tab of the launch profile: it opens now.
                        let key = crate::chrome::open_window_key(pending, &ctx);
                        with_ui(|ui| {
                            if let Some(r) = ui.session.restoring.get_mut(&tag) {
                                r.key = key;
                            }
                        });
                    }
                }
            }),
        );
    }
    changed();
}

/// `register_tab` of a restored tab: remember its id and return its strip position. When
/// both tabs of the window's split exist, the split view is set up again. UI thread.
pub(crate) fn registered(
    ui: &mut crate::bridge::UiState,
    tag: u32,
    index: usize,
    id: TabId,
) -> usize {
    let Some(r) = ui.session.restoring.get_mut(&tag) else {
        return usize::MAX;
    };
    let at = insert_index(&r.ids, index);
    r.ids.insert(index, id);
    if let Some(split) = r.split
        && let (Some(l), Some(rt)) = (r.ids.get(&split.left), r.ids.get(&split.right))
    {
        let (left, right, ratio) = (*l, *rt, split.ratio);
        r.split = None;
        // Outside this borrow and after the tab is shown.
        post_to_ui(Box::new(move || {
            if let Err(err) = crate::chrome::split_tabs(left, right, ratio) {
                tracing::debug!(%err, "restored split not set up");
            }
        }));
    }
    at
}

// ------------------------------------------------------------------ closed tabs

/// Remember a tab that is being closed (window `key`, strip position `index`). UI thread,
/// outside a `UiState` borrow.
pub(crate) fn remember_closed(key: i32, index: usize, id: TabId) {
    with_ui(|ui| {
        let Some(t) = ui.tabs.get(&id) else { return };
        if t.profile.incognito || t.offscreen || t.url.is_empty() || t.url == "about:blank" {
            return;
        }
        let entry = ClosedTab {
            url: t.url.clone(),
            title: t.title.clone(),
            profile: t.profile.id.clone(),
            index,
            window: Some(key),
        };
        push_closed(&mut ui.session.closed, entry);
    });
    changed();
}

/// `tabs.restore_closed`: re-open the most recent closed tab at its old place. `reply`
/// learns the new tab's id; `Ok(false)` = nothing to restore. UI thread.
pub(crate) fn reopen_closed(reply: oneshot::Sender<TabId>) -> Result<bool, BackendError> {
    let ctx = with_ui(|ui| ui.shell.clone()).ok_or(BackendError::ShuttingDown)?;
    let Some(closed) = with_ui(|ui| ui.session.closed.pop()) else {
        return Ok(false);
    };
    changed();
    let profile = ProfileKey::from_options(closed.profile.as_deref(), false).unwrap_or_default();
    let mut pending = PendingTab::new(&closed.url, profile.clone(), None);
    pending.reply = Some(reply);
    pending.index = Some(closed.index);
    let window = closed.window;
    crate::profiles::when_ready(
        &profile,
        Box::new(move || {
            let key = with_ui(|ui| window.filter(|k| ui.windows.contains_key(k)));
            match key {
                Some(key) => crate::chrome::add_tab(&ctx, key, pending),
                None => {
                    pending.index = None;
                    if let Err(err) = crate::chrome::open_tab(pending) {
                        tracing::error!(%err, "closed tab not re-opened");
                    }
                }
            }
        }),
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: i32, url: &str, profile: Option<&str>, incognito: bool) -> LiveTab {
        LiveTab {
            id: TabId(id),
            url: url.into(),
            title: format!("T{id}"),
            profile: profile.map(str::to_string),
            incognito,
        }
    }

    #[test]
    fn snapshot_keeps_windows_tabs_active_and_split() {
        let w = LiveWindow {
            tabs: vec![
                tab(1, "https://a.test/", None, false),
                tab(2, "https://secret.test/", None, true), // incognito: never saved
                tab(3, "https://b.test/", Some("work"), false),
                tab(4, "about:blank", None, false), // nothing to restore
                tab(5, "https://c.test/", None, false),
            ],
            active: Some(TabId(5)),
            split: Some((TabId(3), TabId(5), 0.4)),
            closing: false,
        };
        let file = snapshot(std::slice::from_ref(&w), &[]).unwrap();
        assert_eq!(file.version, 1);
        let saved = &file.windows[0];
        let urls: Vec<&str> = saved.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://a.test/", "https://b.test/", "https://c.test/"]
        );
        assert_eq!(saved.tabs[1].profile.as_deref(), Some("work"));
        assert_eq!(saved.active, 2);
        assert_eq!(
            saved.split,
            Some(SessionSplit {
                left: 1,
                right: 2,
                ratio: 0.4
            })
        );
        // Round trip through JSON.
        let text = serde_json::to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<SessionFile>(&text).unwrap(), file);
        assert!(!text.contains("secret"));
    }

    #[test]
    fn nothing_left_means_no_file() {
        let closing = LiveWindow {
            tabs: vec![tab(1, "https://a.test/", None, false)],
            active: Some(TabId(1)),
            split: None,
            closing: true,
        };
        let only_incognito = LiveWindow {
            tabs: vec![tab(2, "https://a.test/", None, true)],
            active: Some(TabId(2)),
            split: Some((TabId(2), TabId(9), 0.5)),
            closing: false,
        };
        assert_eq!(snapshot(std::slice::from_ref(&closing), &[]), None);
        assert_eq!(snapshot(&[only_incognito], &[]), None);
        // A split with one half not saved is dropped; the other window survives.
        let open = LiveWindow {
            closing: false,
            split: Some((TabId(1), TabId(2), 0.5)),
            tabs: vec![
                tab(1, "https://a.test/", None, false),
                tab(2, "https://b.test/", None, true),
            ],
            ..closing
        };
        let file = snapshot(&[open], &[]).unwrap();
        assert_eq!(file.windows[0].split, None);
        // Old files without the newer fields still parse.
        let old: SessionFile =
            serde_json::from_str(r#"{"windows":[{"tabs":[{"url":"x"}]}]}"#).unwrap();
        assert_eq!(old.windows[0].active, 0);
    }

    #[test]
    fn plan_defers_other_profiles() {
        let window = SessionWindow {
            tabs: vec![
                SessionTab {
                    url: "https://a.test/".into(),
                    ..Default::default()
                },
                SessionTab {
                    url: "https://b.test/".into(),
                    profile: Some("work".into()),
                    ..Default::default()
                },
                SessionTab {
                    url: "https://c.test/".into(),
                    profile: Some("default".into()),
                    ..Default::default()
                },
            ],
            active: 7, // out of range: the last tab
            split: None,
        };
        let plan = plan_window(&window, "default");
        assert_eq!(
            plan.iter().map(|t| t.deferred).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(
            plan[2].profile, None,
            "the launch profile by name is the launch profile"
        );
        assert_eq!(
            plan.iter().map(|t| t.activate).collect::<Vec<_>>(),
            [false, false, true]
        );
    }

    #[test]
    fn restored_tabs_keep_their_order_whatever_registers_first() {
        let mut present = BTreeMap::new();
        // Saved order 0..4; they register as 3, 0, 4, 1, 2.
        let mut strip: Vec<usize> = Vec::new();
        for index in [3, 0, 4, 1, 2] {
            let at = insert_index(&present, index);
            strip.insert(at, index);
            present.insert(index, TabId(index as i32));
        }
        assert_eq!(strip, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn closed_stack_is_bounded() {
        let mut stack = Vec::new();
        for i in 0..(CLOSED_MAX + 3) {
            push_closed(
                &mut stack,
                ClosedTab {
                    url: format!("https://{i}.test/"),
                    ..Default::default()
                },
            );
        }
        assert_eq!(stack.len(), CLOSED_MAX);
        assert_eq!(stack[0].url, "https://3.test/");
        assert_eq!(
            stack.last().unwrap().url,
            format!("https://{}.test/", CLOSED_MAX + 2)
        );
    }
}
