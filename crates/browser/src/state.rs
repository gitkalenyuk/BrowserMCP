//! Per-tab perception state that must survive between command calls.
//!
//! An agent takes a snapshot in one call (`@4:e3 button "Sign in"`) and clicks `@4:e3` in the
//! next one, so the [`RefRegistry`] that knows "e3 = DOM node 1234" has to outlive a single
//! call. [`PerceptionState`] holds one [`TabState`] per tab and lives in the registry's
//! [`bmcp_core::Extensions`] map (put there by [`crate::register`]).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bmcp_core::{Ctx, TabId};
use bmcp_perception::{RefRegistry, Snapshot, SnapshotOptions};

use crate::human::Human;

/// How many recent snapshots per tab `page.diff since=` can compare against.
pub const KEEP_HISTORY: usize = 16;

/// All tabs' state. One instance per browser, shared by every call.
#[derive(Default)]
pub struct PerceptionState {
    tabs: Mutex<HashMap<TabId, Arc<tokio::sync::Mutex<TabState>>>>,
}

impl PerceptionState {
    /// The state of one tab, created on first use.
    ///
    /// It is behind an async (tokio) mutex: a command keeps it locked for its whole run, so two
    /// commands on the same tab never interleave their CDP calls (e.g. two clicks at once).
    pub fn tab(&self, tab: TabId) -> Arc<tokio::sync::Mutex<TabState>> {
        let mut tabs = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        tabs.entry(tab).or_default().clone()
    }

    /// Forget tabs that are not in `open` any more (closed tabs).
    pub fn retain(&self, open: &[TabId]) {
        let mut tabs = self.tabs.lock().unwrap_or_else(|e| e.into_inner());
        tabs.retain(|id, _| open.contains(id));
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
}

impl TabState {
    /// Remember a snapshot as the newest one.
    pub fn remember(&mut self, snap: Snapshot, options: SnapshotOptions) {
        self.history.push_back((snap, options));
        while self.history.len() > KEEP_HISTORY {
            self.history.pop_front();
        }
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
