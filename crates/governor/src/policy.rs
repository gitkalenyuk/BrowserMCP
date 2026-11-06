//! The automatic policy as a pure function: tabs + limits + memory → what to freeze/discard.
//! No clock, no browser; the [`crate::Governor`] collects the inputs every tick.

use std::time::Duration;

use bmcp_core::TabId;

use crate::config::Limits;
use crate::memory::MemInfo;

/// Coarse state of a tab, as the policy sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Live,
    Frozen,
    Discarded,
}

/// One tab as input to [`decide`].
#[derive(Debug, Clone, PartialEq)]
pub struct TabView {
    pub id: TabId,
    pub kind: Kind,
    /// How long it has been in the background without commands (0 for the active tab).
    pub idle: Duration,
    /// A guard said "leave it alone" (active, locked, recording, in use, audio, …).
    pub protected: bool,
    /// A real web page (only those are discarded; see `actions::is_discardable`).
    pub discardable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Freeze,
    Discard,
}

/// One decision with a human-readable reason (shown in `governor.status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub tab: TabId,
    pub action: Action,
    pub reason: String,
}

/// Under memory pressure a tab must have been idle at least this long to be discarded
/// (so switching between two tabs never discards the other one).
pub const PRESSURE_MIN_IDLE: Duration = Duration::from_secs(60);

/// What to do this tick. Rules, in order:
///
/// 1. disabled → nothing; protected tabs are never touched;
/// 2. idle ≥ `discard_after` → discard (web pages only);
///    idle ≥ `freeze_after` and still live → freeze;
/// 3. more than `max_live_tabs` not discarded → discard the most idle candidates until the
///    count fits (protected tabs count, but are never picked);
/// 4. memory pressure and nothing discarded yet this tick → discard the single most idle
///    candidate (idle ≥ [`PRESSURE_MIN_IDLE`]); one per tick, so we free memory step by step.
pub fn decide(tabs: &[TabView], limits: &Limits, mem: MemInfo) -> Vec<Planned> {
    if !limits.enabled {
        return Vec::new();
    }
    let mut plan: Vec<Planned> = Vec::new();

    // 2. Time in the background.
    for t in tabs.iter().filter(|t| !t.protected) {
        if let Some(after) = limits.discard_after()
            && t.discardable
            && t.kind != Kind::Discarded
            && t.idle >= after
        {
            plan.push(planned(
                t.id,
                Action::Discard,
                format!("background {}", human(after)),
            ));
        } else if let Some(after) = limits.freeze_after()
            && t.kind == Kind::Live
            && t.idle >= after
        {
            plan.push(planned(
                t.id,
                Action::Freeze,
                format!("background {}", human(after)),
            ));
        }
    }

    // Candidates for rules 3 and 4: not protected, not yet (planned to be) discarded,
    // most idle first.
    let discarding = |plan: &[Planned], id: TabId| {
        plan.iter()
            .any(|p| p.tab == id && p.action == Action::Discard)
    };
    let mut candidates: Vec<&TabView> = tabs
        .iter()
        .filter(|t| !t.protected && t.discardable && t.kind != Kind::Discarded)
        .collect();
    candidates.sort_by_key(|t| std::cmp::Reverse(t.idle));

    // 3. Too many live tabs.
    if limits.max_live_tabs > 0 {
        let live = tabs
            .iter()
            .filter(|t| t.kind != Kind::Discarded && !discarding(&plan, t.id))
            .count();
        let mut excess = live.saturating_sub(limits.max_live_tabs);
        for t in &candidates {
            if excess == 0 {
                break;
            }
            if discarding(&plan, t.id) {
                continue;
            }
            // A planned freeze becomes a discard.
            plan.retain(|p| p.tab != t.id);
            let reason = format!("more than {} live tabs", limits.max_live_tabs);
            plan.push(planned(t.id, Action::Discard, reason));
            excess -= 1;
        }
    }

    // 4. Memory pressure.
    let any_discard = plan.iter().any(|p| p.action == Action::Discard);
    if mem.under_pressure(limits.pressure_free_pct)
        && !any_discard
        && let Some(t) = candidates.iter().find(|t| t.idle >= PRESSURE_MIN_IDLE)
    {
        plan.retain(|p| p.tab != t.id);
        let reason = format!("memory pressure ({:.0}% RAM free)", mem.free_pct());
        plan.push(planned(t.id, Action::Discard, reason));
    }
    plan
}

fn planned(tab: TabId, action: Action, reason: String) -> Planned {
    Planned {
        tab,
        action,
        reason,
    }
}

/// `300 s` → `5m`, `90 s` → `90s`.
fn human(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;

    const MIN: u64 = 60;

    fn tab(id: i32, kind: Kind, idle_secs: u64) -> TabView {
        TabView {
            id: TabId(id),
            kind,
            idle: Duration::from_secs(idle_secs),
            protected: false,
            discardable: true,
        }
    }

    fn plenty() -> MemInfo {
        MemInfo {
            total: 16 << 30,
            available: 8 << 30,
        }
    }

    fn short(plan: &[Planned]) -> Vec<(i32, Action)> {
        let mut v: Vec<_> = plan.iter().map(|p| (p.tab.0, p.action)).collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }

    /// Normal profile: freeze after 5 min, discard after 30 min.
    #[test]
    fn decision_table_by_idle_time() {
        let limits = Profile::Normal.limits();
        // (state, idle, protected, discardable) → expected action
        let table: &[(Kind, u64, bool, bool, Option<Action>)] = &[
            (Kind::Live, 0, false, true, None),
            (Kind::Live, 4 * MIN, false, true, None),
            (Kind::Live, 5 * MIN, false, true, Some(Action::Freeze)),
            (Kind::Live, 29 * MIN, false, true, Some(Action::Freeze)),
            (Kind::Frozen, 29 * MIN, false, true, None),
            (Kind::Live, 30 * MIN, false, true, Some(Action::Discard)),
            (Kind::Frozen, 30 * MIN, false, true, Some(Action::Discard)),
            (Kind::Discarded, 90 * MIN, false, true, None),
            // Protected tabs are never touched.
            (Kind::Live, 90 * MIN, true, true, None),
            // The new tab page etc. are only frozen.
            (Kind::Live, 90 * MIN, false, false, Some(Action::Freeze)),
            (Kind::Frozen, 90 * MIN, false, false, None),
        ];
        for (i, &(kind, idle, protected, discardable, expected)) in table.iter().enumerate() {
            let view = TabView {
                protected,
                discardable,
                ..tab(1, kind, idle)
            };
            let plan = decide(&[view], &limits, plenty());
            assert_eq!(plan.first().map(|p| p.action), expected, "row {i}");
        }
        let plan = decide(&[tab(1, Kind::Live, 5 * MIN)], &limits, plenty());
        assert_eq!(plan[0].reason, "background 5m");
    }

    #[test]
    fn disabled_or_zero_limits_do_nothing() {
        let mut limits = Profile::Normal.limits();
        limits.enabled = false;
        assert!(decide(&[tab(1, Kind::Live, 999 * MIN)], &limits, plenty()).is_empty());
        let mut limits = Profile::Normal.limits();
        limits.freeze_after_secs = 0;
        limits.discard_after_secs = 0;
        assert!(decide(&[tab(1, Kind::Live, 999 * MIN)], &limits, plenty()).is_empty());
    }

    #[test]
    fn max_live_tabs_discards_the_most_idle() {
        let mut limits = Profile::Low.limits(); // max 8 live
        limits.max_live_tabs = 3;
        let tabs = [
            tab(1, Kind::Live, 0),
            tab(2, Kind::Live, 10),
            tab(3, Kind::Frozen, 50),
            tab(4, Kind::Live, 40),
            TabView {
                protected: true,
                ..tab(5, Kind::Live, 100)
            },
            tab(6, Kind::Discarded, 500),
        ];
        // 5 not discarded, limit 3 → discard the two most idle unprotected: 3 and 4.
        let plan = decide(&tabs, &limits, plenty());
        assert_eq!(
            short(&plan),
            vec![(3, Action::Discard), (4, Action::Discard)]
        );
        assert_eq!(plan[0].reason, "more than 3 live tabs");
    }

    #[test]
    fn max_live_turns_a_planned_freeze_into_a_discard() {
        let mut limits = Profile::Normal.limits();
        limits.max_live_tabs = 1;
        let tabs = [tab(1, Kind::Live, 0), tab(2, Kind::Live, 6 * MIN)];
        assert_eq!(
            short(&decide(&tabs, &limits, plenty())),
            vec![(2, Action::Discard)]
        );
    }

    #[test]
    fn memory_pressure_discards_one_tab_per_tick() {
        let limits = Profile::Normal.limits(); // pressure below 15 % free
        let low = MemInfo {
            total: 100,
            available: 10,
        };
        let tabs = [
            tab(1, Kind::Live, 0),
            tab(2, Kind::Live, 2 * MIN),
            tab(3, Kind::Frozen, 4 * MIN),
            tab(4, Kind::Live, 30), // too recent for pressure
        ];
        let plan = decide(&tabs, &limits, low);
        assert_eq!(short(&plan), vec![(3, Action::Discard)]);
        assert!(plan[0].reason.starts_with("memory pressure (10%"));
        // Enough memory: nothing.
        assert!(decide(&tabs, &limits, plenty()).is_empty());
        // Only recently used tabs: nothing even under pressure.
        assert!(
            decide(
                &[tab(1, Kind::Live, 0), tab(4, Kind::Live, 30)],
                &limits,
                low
            )
            .is_empty()
        );
    }
}
