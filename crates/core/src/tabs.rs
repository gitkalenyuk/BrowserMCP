//! [`TabStrip`]: the order of the tabs in one window and which one is active.
//!
//! Plain data, no CEF: the CEF crate keeps one `TabStrip` per window (and the
//! `FakeBackend` one for tests), so the rules below are unit-tested here once:
//!
//! * new tabs go where the caller asks (end by default; popups right after their opener);
//! * closing the active tab activates its right neighbour, or the left one if it was last
//!   (Chrome's rule, and what the UI's optimistic reducer guesses too);
//! * the strip never holds the same id twice.

use crate::backend::TabId;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabStrip {
    order: Vec<TabId>,
    active: Option<TabId>,
}

/// What [`TabStrip::remove`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    /// Where the tab was.
    pub index: usize,
    /// The tab that became active because the removed one was active (None if the
    /// active tab did not change, or the strip is now empty).
    pub new_active: Option<TabId>,
}

impl TabStrip {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tab ids, left to right.
    pub fn tabs(&self) -> &[TabId] {
        &self.order
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn contains(&self, id: TabId) -> bool {
        self.order.contains(&id)
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.order.iter().position(|t| *t == id)
    }

    /// Add a tab at `index` (clamped to the end; `None` = end) and return the index it got.
    /// Adding an id that is already there only returns its current index.
    /// The first tab of an empty strip becomes active automatically.
    pub fn insert(&mut self, id: TabId, index: Option<usize>) -> usize {
        if let Some(existing) = self.index_of(id) {
            return existing;
        }
        let at = index.unwrap_or(self.order.len()).min(self.order.len());
        self.order.insert(at, id);
        if self.active.is_none() {
            self.active = Some(id);
        }
        at
    }

    /// Make `id` the active tab. Returns false if it is not in this strip.
    pub fn activate(&mut self, id: TabId) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.active = Some(id);
        true
    }

    /// Remove a tab. If it was the active one, its right neighbour (or the left one when
    /// it was the last tab) becomes active. Returns `None` if the tab was not here.
    pub fn remove(&mut self, id: TabId) -> Option<Removed> {
        let index = self.index_of(id)?;
        self.order.remove(index);
        let mut new_active = None;
        if self.active == Some(id) {
            // After the remove, `index` points at the right neighbour (if any).
            self.active = self.order.get(index).or_else(|| self.order.last()).copied();
            new_active = self.active;
        }
        Some(Removed { index, new_active })
    }

    /// The tab `delta` steps away from the active one, wrapping around (Ctrl+Tab).
    pub fn neighbour(&self, delta: isize) -> Option<TabId> {
        let n = self.order.len() as isize;
        if n == 0 {
            return None;
        }
        let current = self
            .active
            .and_then(|id| self.index_of(id))
            .map_or(0, |i| i as isize);
        let next = (current + delta).rem_euclid(n) as usize;
        Some(self.order[next])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(strip: &TabStrip) -> Vec<i32> {
        strip.tabs().iter().map(|t| t.0).collect()
    }

    fn strip(tabs: &[i32]) -> TabStrip {
        let mut s = TabStrip::new();
        for t in tabs {
            s.insert(TabId(*t), None);
        }
        s
    }

    #[test]
    fn insert_order_and_first_tab_is_active() {
        let mut s = TabStrip::new();
        assert_eq!(s.insert(TabId(1), None), 0);
        assert_eq!(s.active(), Some(TabId(1)));
        assert_eq!(s.insert(TabId(2), None), 1);
        // Popup right after its opener (index 1), and a clamped index.
        assert_eq!(s.insert(TabId(3), Some(1)), 1);
        assert_eq!(s.insert(TabId(4), Some(99)), 3);
        assert_eq!(ids(&s), [1, 3, 2, 4]);
        // Still the first tab: inserting does not activate.
        assert_eq!(s.active(), Some(TabId(1)));
        // Duplicate ids are ignored.
        assert_eq!(s.insert(TabId(2), Some(0)), 2);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn activate_only_known_tabs() {
        let mut s = strip(&[1, 2]);
        assert!(s.activate(TabId(2)));
        assert_eq!(s.active(), Some(TabId(2)));
        assert!(!s.activate(TabId(9)));
        assert_eq!(s.active(), Some(TabId(2)));
    }

    #[test]
    fn closing_active_tab_activates_right_then_left_neighbour() {
        let mut s = strip(&[1, 2, 3]);
        s.activate(TabId(2));
        // Middle tab: the right neighbour wins.
        assert_eq!(
            s.remove(TabId(2)),
            Some(Removed {
                index: 1,
                new_active: Some(TabId(3))
            })
        );
        // Last tab: the left neighbour.
        assert_eq!(
            s.remove(TabId(3)),
            Some(Removed {
                index: 1,
                new_active: Some(TabId(1))
            })
        );
        // Only tab: strip becomes empty, nothing active.
        assert_eq!(
            s.remove(TabId(1)),
            Some(Removed {
                index: 0,
                new_active: None
            })
        );
        assert!(s.is_empty());
        assert_eq!(s.active(), None);
    }

    #[test]
    fn closing_inactive_tab_keeps_active() {
        let mut s = strip(&[1, 2, 3]);
        s.activate(TabId(3));
        assert_eq!(
            s.remove(TabId(1)),
            Some(Removed {
                index: 0,
                new_active: None
            })
        );
        assert_eq!(s.active(), Some(TabId(3)));
        assert_eq!(s.remove(TabId(9)), None);
    }

    #[test]
    fn neighbour_wraps_around() {
        let mut s = strip(&[1, 2, 3]);
        assert_eq!(s.neighbour(1), Some(TabId(2)));
        assert_eq!(s.neighbour(-1), Some(TabId(3)));
        s.activate(TabId(3));
        assert_eq!(s.neighbour(1), Some(TabId(1)));
        assert_eq!(TabStrip::new().neighbour(1), None);
    }
}
