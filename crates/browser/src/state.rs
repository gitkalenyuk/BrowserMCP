//! Per-tab perception state that must survive between command calls.
//!
//! An agent takes a snapshot in one call (`@4:e3 button "Sign in"`) and clicks `@4:e3` in the
//! next one, so the [`RefRegistry`] that knows "e3 = DOM node 1234" has to outlive a single
//! call. [`PerceptionState`] holds one [`TabState`] per tab and lives in the registry's
//! [`bmcp_core::Extensions`] map (put there by [`crate::register`]).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bmcp_core::{Ctx, TabId};
use bmcp_perception::{RefRegistry, RefSpace, Snapshot, SnapshotOptions};

use crate::frames::Frames;
use crate::human::Human;

/// How many recent snapshots per tab `page.diff since=` can compare against.
pub const KEEP_HISTORY: usize = 16;
/// Rough memory limit for one tab's snapshot history. `page.act` keeps full-page snapshots
/// (all text, no budget) for its diffs, so 16 of a huge page can be a lot; past this the
/// oldest ones go (the newest one always stays).
pub const KEEP_HISTORY_BYTES: usize = 8 << 20;

/// All tabs' state. One instance per browser, shared by every call.
#[derive(Default)]
pub struct PerceptionState {
    tabs: Mutex<HashMap<TabId, Arc<tokio::sync::Mutex<TabState>>>>,
    /// Snapshot ids and e-numbers shared by all tabs, so a ref names one node in one tab.
    space: Arc<RefSpace>,
}

impl PerceptionState {
    /// The state of one tab, created on first use.
    ///
    /// It is behind an async (tokio) mutex: a command keeps it locked for its whole run, so two
    /// commands on the same tab never interleave their CDP calls (e.g. two clicks at once).
    pub fn tab(&self, tab: TabId) -> Arc<tokio::sync::Mutex<TabState>> {
        let mut tabs = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        tabs.entry(tab)
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(TabState::with_space(
                    self.space.clone(),
                )))
            })
            .clone()
    }

    /// Forget tabs that are not in `open` any more (closed tabs).
    pub fn retain(&self, open: &[TabId]) {
        let mut tabs = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        tabs.retain(|id, _| open.contains(id));
    }

    /// Tabs we keep state for, in no particular order.
    pub fn tracked_tabs(&self) -> Vec<TabId> {
        let tabs = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        tabs.keys().copied().collect()
    }

    /// Which other tab handed out `input` (a ref that just failed in `tab`), if any.
    ///
    /// Tabs busy with another command are skipped (we do not wait for them), so `None` only
    /// means "no idle tab claims it".
    pub fn owner_of(&self, tab: TabId, input: &str) -> Option<TabId> {
        let tabs = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        tabs.iter()
            .filter(|(id, _)| **id != tab)
            .find(|(_, state)| {
                state
                    .try_lock()
                    .is_ok_and(|state| state.registry.issued(input))
            })
            .map(|(id, _)| *id)
    }

    /// The shared instance from the call's context (created on first use, so tests that build a
    /// bare registry work too).
    pub fn from_ctx(ctx: &Ctx) -> Arc<PerceptionState> {
        ctx.extensions.get_or_insert_with(PerceptionState::default)
    }
}

/// What we remember about one tab.
#[derive(Default)]
pub struct TabState {
    /// Ref numbers (`e3`) ↔ DOM nodes for the current document.
    pub registry: RefRegistry,
    /// CDP `loaderId` of the main frame when we last looked. A different value means the
    /// tab loaded a new document, so all old refs are stale.
    pub loader_id: Option<String>,
    /// Recent snapshots (oldest first) with the options they were taken with. Diffs only make
    /// sense between snapshots taken with the same options.
    pub history: VecDeque<(Snapshot, SnapshotOptions)>,
    /// Human-like input settings (`input.humanlike`); `None` = the fast path.
    pub human: Option<Human>,
    /// Where we last moved the mouse (viewport CSS pixels); human-like moves start here.
    pub mouse: Option<(f64, f64)>,
    /// The tab's out-of-process iframes and where their nodes live.
    pub frames: Frames,
}

impl TabState {
    /// A tab whose refs come from `space` (shared with the other tabs).
    pub fn with_space(space: Arc<RefSpace>) -> TabState {
        TabState {
            registry: RefRegistry::with_space(space),
            ..TabState::default()
        }
    }

    /// Remember a snapshot as the newest one. Keeps at most [`KEEP_HISTORY`] snapshots and
    /// about [`KEEP_HISTORY_BYTES`] of them.
    pub fn remember(&mut self, snap: Snapshot, options: SnapshotOptions) {
        self.history.push_back((snap, options));
        while self.history.len() > KEEP_HISTORY
            || (self.history.len() > 1 && self.history_bytes() > KEEP_HISTORY_BYTES)
        {
            self.history.pop_front();
        }
    }

    /// Rough size of the remembered snapshots in bytes.
    pub fn history_bytes(&self) -> usize {
        self.history
            .iter()
            .map(|(snap, _)| snapshot_bytes(snap))
            .sum()
    }

    /// The newest snapshot of the current document, with its options.
    pub fn latest(&self) -> Option<&(Snapshot, SnapshotOptions)> {
        self.history
            .back()
            .filter(|(snap, _)| snap.document == self.registry.document())
    }

    /// A remembered snapshot by id.
    pub fn by_id(&self, id: u64) -> Option<&(Snapshot, SnapshotOptions)> {
        self.history.iter().find(|(snap, _)| snap.id == id)
    }

    /// Role and name of a DOM node as printed in the newest snapshot that shows it,
    /// e.g. `button "Sign in"`. Used in messages.
    pub fn describe(&self, backend_node_id: i64) -> Option<String> {
        self.history.iter().rev().find_map(|(snap, _)| {
            let node = snap
                .nodes
                .iter()
                .find(|n| n.backend_node_id == backend_node_id)?;
            Some(if node.name.is_empty() {
                node.role.clone()
            } else {
                format!("{} \"{}\"", node.role, node.name)
            })
        })
    }
}

/// Rough heap size of a snapshot: its text plus its node and ref lists (strings plus a
/// fixed guess for the struct and allocation overhead).
fn snapshot_bytes(snap: &Snapshot) -> usize {
    const OVERHEAD: usize = 64;
    let nodes: usize = snap
        .nodes
        .iter()
        .map(|n| {
            let states: usize = n.states.iter().map(|s| s.len() + 24).sum();
            OVERHEAD + n.role.len() + n.name.len() + states
        })
        .sum();
    let refs: usize = snap
        .refs
        .iter()
        .map(|r| OVERHEAD + r.ref_str.len() + r.role.len() + r.name.len())
        .sum();
    snap.text.len() + nodes + refs
}
