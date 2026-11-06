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
//! When the page navigates the caller calls [`RefRegistry::new_document`]. Old refs then
//! fail with `STALE_REF` instead of silently pointing at some other node. E-numbers keep
//! counting up across documents (they are never reused within one tab), which is what
//! lets us tell a stale bare `e3` from a typo.

use std::collections::{BTreeMap, HashMap};

use crate::error::{PerceptionError, Result};

/// How many snapshot records we remember per tab. Older `@S:` refs become stale.
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

/// The e-number tables. Kept separate so a snapshot can work on a copy and only
/// commit it when the text is final (see `snapshot.rs`).
#[derive(Debug, Clone, Default)]
pub(crate) struct ElementIds {
    by_backend: HashMap<i64, u32>,
    by_number: HashMap<u32, i64>,
    /// Last number handed out (0 = none yet).
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
#[derive(Debug, Clone, Default)]
pub struct RefRegistry {
    /// Document generation, bumped by `new_document`.
    document: u64,
    /// E-numbers below this were handed out in an older document.
    first_number_of_document: u32,
    pub(crate) ids: ElementIds,
    /// Last snapshot id handed out (0 = none yet).
    last_snap: u64,
    snaps: BTreeMap<u64, SnapRecord>,
}

impl RefRegistry {
    pub fn new() -> RefRegistry {
        RefRegistry {
            first_number_of_document: 1,
            ..RefRegistry::default()
        }
    }

    /// Call when the tab's main frame navigates to a new document. All existing refs
    /// become stale; snapshot ids keep increasing.
    pub fn new_document(&mut self) {
        self.document += 1;
        self.ids.by_backend.clear();
        self.ids.by_number.clear();
        self.first_number_of_document = self.ids.last + 1;
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

    /// Element ref -> `backendDOMNodeId`. See [`resolve`].
    pub fn resolve(&self, input: &str) -> Result<i64> {
        let parsed = parse_ref(input)?;
        if parsed.kind != RefKind::Element {
            return Err(PerceptionError::BadRef {
                input: input.to_string(),
                reason: "g-refs name collapsed groups; use them as root= of a snapshot".to_string(),
            });
        }
        self.check_snapshot(input, parsed.snap)?;
        match self.ids.by_number.get(&parsed.number) {
            Some(&backend) => Ok(backend),
            None if parsed.number < self.first_number_of_document => {
                Err(PerceptionError::StaleRef {
                    input: input.to_string(),
                    reason: "the page navigated since this ref was issued; take a new snapshot"
                        .to_string(),
                })
            }
            None => Err(PerceptionError::BadRef {
                input: input.to_string(),
                reason: "no such ref in the current page; take a snapshot or use find".to_string(),
            }),
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
        let record = self.check_snapshot(input, parsed.snap)?;
        record
            .groups
            .get(parsed.number as usize - 1)
            .cloned()
            .ok_or_else(|| PerceptionError::BadRef {
                input: input.to_string(),
                reason: "that snapshot has no such group".to_string(),
            })
    }

    /// Find the snapshot a ref belongs to and make sure it is from the current document.
    fn check_snapshot(&self, input: &str, snap: Option<u64>) -> Result<&SnapRecord> {
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
            None if id <= self.last_snap => {
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

    /// Hand out the next snapshot id.
    pub(crate) fn begin_snapshot(&mut self) -> u64 {
        self.last_snap += 1;
        self.last_snap
    }

    /// Remember a finished snapshot and forget the oldest ones.
    pub(crate) fn finish_snapshot(&mut self, id: u64, groups: Vec<GroupTarget>) {
        let record = SnapRecord {
            document: self.document,
            groups,
        };
        self.snaps.insert(id, record);
        while self.snaps.len() > KEEP_SNAPSHOTS {
            self.snaps.pop_first();
        }
    }
}

/// Resolve an element ref (`@7:e3` or bare `e3`) to its `backendDOMNodeId`.
///
/// Errors: `BadRef` (malformed or unknown), `StaleRef` (page navigated, or the snapshot
/// is too old to remember).
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
        let id = registry.begin_snapshot();
        assert_eq!(registry.ids.number_for(100), 1);
        assert_eq!(registry.ids.number_for(200), 2);
        assert_eq!(registry.ids.number_for(100), 1);
        registry.finish_snapshot(id, Vec::new());
        assert_eq!(registry.resolve("e2"), Ok(200));

        registry.new_document();
        let id = registry.begin_snapshot();
        // Same backend id in the new document gets a fresh number.
        assert_eq!(registry.ids.number_for(100), 3);
        registry.finish_snapshot(id, Vec::new());
        assert_eq!(registry.resolve("e2").unwrap_err().code(), "STALE_REF");
        assert_eq!(registry.resolve("@1:e1").unwrap_err().code(), "STALE_REF");
        assert_eq!(registry.resolve("e3"), Ok(100));
        assert_eq!(registry.resolve("e9").unwrap_err().code(), "BAD_REF");
    }

    #[test]
    fn old_snapshots_are_forgotten() {
        let mut registry = RefRegistry::new();
        for _ in 0..(KEEP_SNAPSHOTS + 5) {
            let id = registry.begin_snapshot();
            registry.ids.number_for(1);
            registry.finish_snapshot(id, Vec::new());
        }
        assert_eq!(registry.resolve("@1:e1").unwrap_err().code(), "STALE_REF");
        assert_eq!(registry.resolve("@999:e1").unwrap_err().code(), "BAD_REF");
        assert_eq!(registry.resolve("e1"), Ok(1));
    }
}
