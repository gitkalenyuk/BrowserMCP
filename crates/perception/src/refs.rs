//! Refs: the short handles (`@7:e3`, `e3`, `@7:g1`) an agent uses to point at nodes.
//!
//! * `eN` (element ref) names one DOM node. The number is tied to the node's
//!   `backendDOMNodeId` and stays the same in every snapshot of the same document, so
//!   the agent can keep using `e3` after a diff.
//! * `gN` (group ref) names a collapsed part of one snapshot (a long list, or a subtree
//!   hidden by skeleton mode). It is only meaningful as `root=` for a drill-down.
//! * `@S:` says which snapshot the ref came from. Bare refs (`e3`) mean "the latest
//!   snapshot".
//!
//! Snapshot ids and e-numbers come from a [`RefSpace`]. Registries that share one (one
//! registry per tab, one space per browser) never hand out the same snapshot id or
//! e-number twice, so a ref from one tab can never silently name a node in another tab.
//!
//! When the page navigates the caller calls [`RefRegistry::new_document`]. Old refs then
//! fail with `STALE_REF` instead of silently pointing at some other node. E-numbers keep
//! counting up across documents (they are never reused), which is what lets us tell a
//! stale bare `e3` from a typo.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::{PerceptionError, Result};

/// How many snapshot records we remember per tab. Only group refs (`@S:gN`) need the
/// record; element refs keep working as long as the document is unchanged.
pub const KEEP_SNAPSHOTS: usize = 64;

/// Element ref (`e`) or group ref (`g`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Element,
    Group,
}

/// A parsed ref string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedRef {
    /// Snapshot id from `@S:`; `None` for bare refs.
    pub snap: Option<u64>,
    pub kind: RefKind,
    pub number: u32,
}

/// Parse `@7:e3`, `7:e3`, `e3`, `@e3`, `@7:g1` or `g1`.
pub fn parse_ref(input: &str) -> Result<ParsedRef> {
    let bad = |reason: &str| PerceptionError::BadRef {
        input: input.to_string(),
        reason: reason.to_string(),
    };
    let text = input.trim();
    let text = text.strip_prefix('@').unwrap_or(text);

    // Split off the optional "7:" snapshot prefix.
    let (snap, rest) = match text.split_once(':') {
        Some((snap_text, rest)) => {
            let snap: u64 = snap_text
                .parse()
                .map_err(|_| bad("expected a snapshot number before ':' as in @7:e3"))?;
            (Some(snap), rest)
        }
        None => (None, text),
    };

    let kind = if rest.starts_with('e') {
        RefKind::Element
    } else if rest.starts_with('g') {
        RefKind::Group
    } else {
        return Err(bad("refs look like @7:e3, e3 or @7:g1"));
    };
    // `rest` starts with an ASCII letter, so byte 1 is a character boundary.
    let digits = &rest[1..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad("expected a number after 'e' or 'g', as in e3"));
    }
    let number: u32 = digits.parse().map_err(|_| bad("ref number is too large"))?;
    if number == 0 {
        return Err(bad("ref numbers start at 1"));
    }
    Ok(ParsedRef { snap, kind, number })
}

/// What a group ref (`@7:g1`) points at: the container to use as drill-down root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupTarget {
    /// The container's DOM node, when it has one (preferred for lookups: stable).
    pub backend_node_id: Option<i64>,
    /// The container's AX node id (fallback when there is no DOM node).
    pub ax_node_id: String,
    /// Printed role and name, for messages.
    pub role: String,
    pub name: String,
}

/// The numbers handed out so far in one [`RefSpace`].
#[derive(Debug, Default)]
pub(crate) struct Counters {
    /// Last snapshot id handed out (0 = none yet).
    last_snap: u64,
    /// Last e-number handed out (0 = none yet).
    last_number: u32,
}

/// Snapshot ids and e-numbers shared by several registries (one per tab), so refs are
/// unique across tabs. Create one per browser and pass it to
/// [`RefRegistry::with_space`]. A registry made with [`RefRegistry::new`] has its own.
#[derive(Debug, Default)]
pub struct RefSpace {
    counters: Mutex<Counters>,
}

impl RefSpace {
    pub fn new() -> Arc<RefSpace> {
        Arc::new(RefSpace::default())
    }

    /// Exclusive use of the counters while a snapshot hands out numbers. Snapshots of two
    /// tabs taken at the same time on two threads then never get the same numbers.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Counters> {
        self.counters.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Highest snapshot id handed out by any registry of this space.
    pub fn last_snapshot(&self) -> u64 {
        self.lock().last_snap
    }
}

/// The e-number tables. Kept separate so a snapshot can work on a copy and only
/// commit it when the text is final (see `snapshot.rs`).
#[derive(Debug, Clone, Default)]
pub(crate) struct ElementIds {
    by_backend: HashMap<i64, u32>,
    by_number: HashMap<u32, i64>,
    /// Last number handed out in the whole [`RefSpace`] (0 = none yet). Synced from the
    /// space when a snapshot begins and written back when it finishes.
    last: u32,
}

impl ElementIds {
    /// The e-number for a DOM node, handing out a new one the first time.
    pub(crate) fn number_for(&mut self, backend_node_id: i64) -> u32 {
        if let Some(&n) = self.by_backend.get(&backend_node_id) {
            return n;
        }
        self.last += 1;
        self.by_backend.insert(backend_node_id, self.last);
        self.by_number.insert(self.last, backend_node_id);
        self.last
    }
}

/// What we remember about each snapshot (and each `find`, which also gets an id).
#[derive(Debug, Clone)]
struct SnapRecord {
    /// Document generation the snapshot was taken in.
    document: u64,
    /// Group targets; `g1` is `groups[0]`.
    groups: Vec<GroupTarget>,
}

/// Per-tab ref bookkeeping. Create one per tab and keep it for the tab's lifetime.
#[derive(Debug, Clone)]
pub struct RefRegistry {
    /// Where snapshot ids and e-numbers come from (shared with the other tabs).
    space: Arc<RefSpace>,
    /// Document generation, bumped by `new_document`.
    document: u64,
    /// E-numbers below this were handed out before the current document (by this tab
    /// in an older document, or by another tab).
    first_number_of_document: u32,
    pub(crate) ids: ElementIds,
    /// Last snapshot id this registry handed out (0 = none yet).
    last_snap: u64,
    snaps: BTreeMap<u64, SnapRecord>,
}

impl Default for RefRegistry {
    fn default() -> RefRegistry {
        RefRegistry::new()
    }
}

impl RefRegistry {
    /// A registry with its own [`RefSpace`] (numbers start at 1).
    pub fn new() -> RefRegistry {
        RefRegistry::with_space(RefSpace::new())
    }

    /// A registry that takes its numbers from a shared space (use one space for all tabs).
    pub fn with_space(space: Arc<RefSpace>) -> RefRegistry {
        RefRegistry {
            space,
            document: 0,
            first_number_of_document: 1,
            ids: ElementIds::default(),
            last_snap: 0,
            snaps: BTreeMap::new(),
        }
    }

    /// The space this registry takes its numbers from.
    pub(crate) fn space(&self) -> Arc<RefSpace> {
        self.space.clone()
    }

    /// Call when the tab's main frame navigates to a new document. All existing refs
    /// become stale; snapshot ids keep increasing.
    pub fn new_document(&mut self) {
        self.document += 1;
        self.ids.by_backend.clear();
        self.ids.by_number.clear();
        // Every number handed out so far (here or in another tab) is from an older document.
        let last = self.ids.last.max(self.space.lock().last_number);
        self.first_number_of_document = last + 1;
    }

    /// Current document generation (0 for the first document).
    pub fn document(&self) -> u64 {
        self.document
    }

    /// Id of the latest snapshot (or find), if any.
    pub fn latest_snapshot(&self) -> Option<u64> {
        if self.last_snap == 0 {
            None
        } else {
            Some(self.last_snap)
        }
    }

    /// The e-number already assigned to a DOM node in this document, if any.
    pub fn number_of(&self, backend_node_id: i64) -> Option<u32> {
        self.ids.by_backend.get(&backend_node_id).copied()
    }

    /// True when this registry handed out `input` and it still means something here: an
    /// element ref whose number is in the current document (or whose `@S:` snapshot is
    /// remembered), or a group ref whose `@S:` snapshot is remembered. Callers use it to
    /// tell the agent which tab a ref that failed elsewhere belongs to.
    pub fn issued(&self, input: &str) -> bool {
        let Ok(parsed) = parse_ref(input) else {
            return false;
        };
        let known_snap = parsed.snap.is_some_and(|id| self.snaps.contains_key(&id));
        match parsed.kind {
            RefKind::Element => self.ids.by_number.contains_key(&parsed.number) || known_snap,
            RefKind::Group => known_snap,
        }
    }

    /// Element ref -> `backendDOMNodeId`. See [`resolve`].
    pub fn resolve(&self, input: &str) -> Result<i64> {
        let parsed = parse_ref(input)?;
        if parsed.kind != RefKind::Element {
            return Err(PerceptionError::BadRef {
                input: input.to_string(),
                reason: "g-refs name collapsed groups; use them as root= of a snapshot".to_string(),
            });
        }
        let stale = |reason: &str| PerceptionError::StaleRef {
            input: input.to_string(),
            reason: reason.to_string(),
        };
        let bad = |reason: &str| PerceptionError::BadRef {
            input: input.to_string(),
            reason: reason.to_string(),
        };
        if self.last_snap == 0 {
            return Err(bad("no snapshot taken yet; take a snapshot first"));
        }
        if let Some(id) = parsed.snap {
            match self.snaps.get(&id) {
                Some(record) if record.document != self.document => {
                    return Err(stale(
                        "the page navigated since this snapshot; take a new snapshot",
                    ));
                }
                Some(_) => {}
                None if id > self.space.last_snapshot() => {
                    return Err(bad("no snapshot with that number yet"));
                }
                // Forgotten (many snapshots ago) or not ours: the e-number alone decides,
                // because e-numbers are unique across documents and tabs.
                None => {}
            }
        }
        match self.ids.by_number.get(&parsed.number) {
            Some(&backend) => Ok(backend),
            None if parsed.number < self.first_number_of_document => Err(stale(
                "the page navigated since this ref was issued; take a new snapshot",
            )),
            None => Err(bad(
                "no such ref in the current page; take a snapshot or use find",
            )),
        }
    }

    /// Group ref -> the collapsed container. See [`resolve_group`].
    pub fn resolve_group(&self, input: &str) -> Result<GroupTarget> {
        let parsed = parse_ref(input)?;
        if parsed.kind != RefKind::Group {
            return Err(PerceptionError::BadRef {
                input: input.to_string(),
                reason: "expected a group ref such as @7:g1".to_string(),
            });
        }
        let record = self.group_snapshot(input, parsed.snap)?;
        record
            .groups
            .get(parsed.number as usize - 1)
            .cloned()
            .ok_or_else(|| PerceptionError::BadRef {
                input: input.to_string(),
                reason: "that snapshot has no such group".to_string(),
            })
    }

    /// Find the snapshot a group ref belongs to and make sure it is from the current
    /// document. Groups only exist in the record, so a forgotten snapshot is an error here.
    fn group_snapshot(&self, input: &str, snap: Option<u64>) -> Result<&SnapRecord> {
        let stale = |reason: &str| PerceptionError::StaleRef {
            input: input.to_string(),
            reason: reason.to_string(),
        };
        let bad = |reason: &str| PerceptionError::BadRef {
            input: input.to_string(),
            reason: reason.to_string(),
        };
        let id = match snap {
            Some(id) => id,
            None => self
                .latest_snapshot()
                .ok_or_else(|| bad("no snapshot taken yet; take a snapshot first"))?,
        };
        let record = match self.snaps.get(&id) {
            Some(record) => record,
            None if id <= self.space.last_snapshot() => {
                return Err(stale("that snapshot is too old; take a new snapshot"));
            }
            None => return Err(bad("no snapshot with that number yet")),
        };
        if record.document != self.document {
            return Err(stale(
                "the page navigated since this snapshot; take a new snapshot",
            ));
        }
        Ok(record)
    }

    /// Hand out the next snapshot id. `counters` is the locked [`RefSpace`]; keep it
    /// locked until [`finish_snapshot`](Self::finish_snapshot).
    pub(crate) fn begin_snapshot(&mut self, counters: &mut Counters) -> u64 {
        counters.last_snap += 1;
        self.last_snap = counters.last_snap;
        // New e-numbers continue after the ones every other tab handed out.
        self.ids.last = self.ids.last.max(counters.last_number);
        self.last_snap
    }

    /// Remember a finished snapshot, publish the e-numbers it handed out to the space and
    /// forget old records.
    pub(crate) fn finish_snapshot(
        &mut self,
        counters: &mut Counters,
        id: u64,
        groups: Vec<GroupTarget>,
    ) {
        counters.last_number = counters.last_number.max(self.ids.last);
        let record = SnapRecord {
            document: self.document,
            groups,
        };
        self.snaps.insert(id, record);
        while self.snaps.len() > KEEP_SNAPSHOTS {
            // Only group refs need a record, so forget the oldest record without groups
            // first. Every page.act takes such snapshots for its diff; this way they do not
            // push out the snapshot whose `@S:gN` the agent is still using.
            let oldest = self
                .snaps
                .iter()
                .find(|(_, record)| record.groups.is_empty())
                .map(|(id, _)| *id)
                .or_else(|| self.snaps.keys().next().copied());
            if let Some(oldest) = oldest {
                self.snaps.remove(&oldest);
            }
        }
    }
}

/// Resolve an element ref (`@7:e3` or bare `e3`) to its `backendDOMNodeId`.
///
/// Errors: `BadRef` (malformed or unknown), `StaleRef` (page navigated since the ref was
/// issued).
pub fn resolve(registry: &RefRegistry, input: &str) -> Result<i64> {
    registry.resolve(input)
}

/// Resolve a group ref (`@7:g1` or bare `g1`) to the container it collapsed.
pub fn resolve_group(registry: &RefRegistry, input: &str) -> Result<GroupTarget> {
    registry.resolve_group(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One fake snapshot: number `backends` (in order) and remember `groups`.
    /// Returns the snapshot id and the numbers handed out.
    fn take(registry: &mut RefRegistry, backends: &[i64], groups: usize) -> (u64, Vec<u32>) {
        let space = registry.space();
        let mut counters = space.lock();
        let id = registry.begin_snapshot(&mut counters);
        let numbers = backends
            .iter()
            .map(|b| registry.ids.number_for(*b))
            .collect();
        let groups = (0..groups)
            .map(|i| GroupTarget {
                backend_node_id: Some(1000 + i as i64),
                ax_node_id: format!("ax{i}"),
                role: "list".into(),
                name: String::new(),
            })
            .collect();
        registry.finish_snapshot(&mut counters, id, groups);
        (id, numbers)
    }

    #[test]
    fn parses_ref_forms() {
        let full = parse_ref("@7:e3").unwrap();
        assert_eq!(full.snap, Some(7));
        assert_eq!(full.kind, RefKind::Element);
        assert_eq!(full.number, 3);
        assert_eq!(parse_ref("e3").unwrap().snap, None);
        assert_eq!(parse_ref(" @e12 ").unwrap().number, 12);
        assert_eq!(parse_ref("@7:g2").unwrap().kind, RefKind::Group);
        for bad in ["", "@", "x3", "e", "e0", "@a:e1", "@7:", "e3x", "@7:e-1"] {
            assert!(parse_ref(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn numbers_survive_documents_without_reuse() {
        let mut registry = RefRegistry::new();
        let (_, numbers) = take(&mut registry, &[100, 200, 100], 0);
        assert_eq!(numbers, [1, 2, 1]);
        assert_eq!(registry.resolve("e2"), Ok(200));

        registry.new_document();
        // Same backend id in the new document gets a fresh number.
        let (_, numbers) = take(&mut registry, &[100], 0);
        assert_eq!(numbers, [3]);
        assert_eq!(registry.resolve("e2").unwrap_err().code(), "STALE_REF");
        assert_eq!(registry.resolve("@1:e1").unwrap_err().code(), "STALE_REF");
        assert_eq!(registry.resolve("e3"), Ok(100));
        assert_eq!(registry.resolve("e9").unwrap_err().code(), "BAD_REF");
    }

    #[test]
    fn element_refs_outlive_forgotten_snapshot_records() {
        // Regression: `@S:eN` went STALE once S fell out of the 64 remembered records,
        // although the page never changed (about 32 page.act calls, 2 snapshots each).
        let mut registry = RefRegistry::new();
        for _ in 0..(KEEP_SNAPSHOTS + 5) {
            take(&mut registry, &[1], 0);
        }
        assert_eq!(registry.resolve("@1:e1"), Ok(1));
        assert_eq!(registry.resolve("e1"), Ok(1));
        assert_eq!(registry.resolve("@999:e1").unwrap_err().code(), "BAD_REF");
        // After a navigation the forgotten snapshot's refs are stale, as they should be.
        registry.new_document();
        take(&mut registry, &[1], 0);
        assert_eq!(registry.resolve("@1:e1").unwrap_err().code(), "STALE_REF");
    }

    #[test]
    fn group_refs_survive_snapshots_without_groups() {
        let mut registry = RefRegistry::new();
        let (with_groups, _) = take(&mut registry, &[1], 2);
        // Many diff-like snapshots (no groups) later the group ref still works.
        for _ in 0..(KEEP_SNAPSHOTS * 3) {
            take(&mut registry, &[1], 0);
        }
        let group = registry
            .resolve_group(&format!("@{with_groups}:g2"))
            .unwrap();
        assert_eq!(group.backend_node_id, Some(1001));
        // Too many snapshots *with* groups do push it out.
        for _ in 0..(KEEP_SNAPSHOTS + 1) {
            take(&mut registry, &[1], 1);
        }
        let err = registry
            .resolve_group(&format!("@{with_groups}:g2"))
            .unwrap_err();
        assert_eq!(err.code(), "STALE_REF");
    }

    #[test]
    fn tabs_sharing_a_space_never_share_refs() {
        // Regression: every tab started at snapshot 1 / e1, so `@1:e5` from tab A
        // silently resolved to some other node in tab B.
        let space = RefSpace::new();
        let mut tab_a = RefRegistry::with_space(space.clone());
        let mut tab_b = RefRegistry::with_space(space.clone());
        let (snap_a, numbers_a) = take(&mut tab_a, &[10, 11], 0);
        // Tab B shows the same DOM node ids (another copy of the same page).
        let (snap_b, numbers_b) = take(&mut tab_b, &[10, 11], 0);
        assert_eq!((snap_a, snap_b), (1, 2));
        assert_eq!(numbers_a, [1, 2]);
        assert_eq!(numbers_b, [3, 4]);

        let a_ref = format!("@{snap_a}:e{}", numbers_a[0]);
        assert_eq!(tab_a.resolve(&a_ref), Ok(10));
        // In tab B the ref fails instead of resolving, and tab A says it is its own.
        assert!(tab_b.resolve(&a_ref).is_err());
        assert!(tab_b.resolve("e1").is_err());
        assert!(tab_a.issued(&a_ref) && tab_a.issued("e1"));
        assert!(!tab_b.issued(&a_ref) && !tab_b.issued("e1"));
        assert!(tab_b.issued("e3"));
        // A navigation in tab B does not make tab A's refs look like B's old ones.
        tab_b.new_document();
        take(&mut tab_b, &[10], 0);
        assert_eq!(tab_a.resolve("e2"), Ok(11));
    }
}
