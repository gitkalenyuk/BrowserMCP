//! Anti-spam: a 1000-row run with failing rows must not send 1000 messages.
//!
//! Three mechanisms, all driven by explicit `now_ms` so tests need no clock:
//!
//! * **dedup** — the same title + text within `dedup_secs` is dropped;
//! * **aggregation** — for `aggregate` rules, the first event of a group (e.g. failures of
//!   run `r12`) goes out at once and opens a window of `aggregate_secs`; later events in the
//!   window are only counted. When the window ends with a count, one summary
//!   ("Run r12 row failures: 37 more in the last 60s") goes out and a new window starts;
//!   a window that ends with nothing new closes the group;
//! * **rate limit** — at most `max_per_minute` notifications per channel; the rest are
//!   dropped and the next one that gets through says "(+N more suppressed)".
//!
//! Questions (notifications with buttons) skip all three: they must always arrive.

use std::collections::{HashMap, VecDeque};

use crate::config::{Channel, Limits};
use crate::model::Notification;

#[derive(Default)]
pub struct Limiter {
    /// dedup key → when it was last sent.
    recent: HashMap<String, u64>,
    groups: HashMap<String, Group>,
    /// Per channel: send times within the last minute.
    sent: HashMap<Channel, VecDeque<u64>>,
    /// Per channel: dropped by the rate limit since the last one that went through.
    suppressed: HashMap<Channel, u32>,
}

struct Group {
    window_start: u64,
    /// Events absorbed in the current window.
    extra: u32,
    /// The latest absorbed event (shown in the summary).
    last: Option<Notification>,
    label: String,
    channels: Vec<Channel>,
}

impl Limiter {
    /// False when the same text was sent within `dedup_secs`.
    pub fn dedup(&mut self, n: &Notification, now_ms: u64, limits: &Limits) -> bool {
        if n.is_question() {
            return true;
        }
        let window = limits.dedup_secs * 1000;
        self.recent
            .retain(|_, at| now_ms.saturating_sub(*at) < window);
        let key = format!("{}\n{}", n.title, n.body);
        if self.recent.contains_key(&key) {
            return false;
        }
        self.recent.insert(key, now_ms);
        true
    }

    /// For aggregated rules: `true` = send it now (first of its window), `false` = absorbed
    /// into the group's next summary. Notifications without a group always go out.
    pub fn aggregate(&mut self, n: &Notification, channels: &[Channel], now_ms: u64) -> bool {
        let Some((key, label)) = &n.group else {
            return true;
        };
        // An open group absorbs; `flush` ends windows and closes quiet groups.
        match self.groups.get_mut(key) {
            Some(group) => {
                group.extra += 1;
                group.last = Some(n.clone());
                false
            }
            None => {
                self.groups.insert(
                    key.clone(),
                    Group {
                        window_start: now_ms,
                        extra: 0,
                        last: None,
                        label: label.clone(),
                        channels: channels.to_vec(),
                    },
                );
                true
            }
        }
    }

    /// Summaries of the windows that have ended (call about once a second).
    pub fn flush(&mut self, now_ms: u64, limits: &Limits) -> Vec<(Notification, Vec<Channel>)> {
        let window = limits.aggregate_secs * 1000;
        let mut out = Vec::new();
        self.groups.retain(|_, group| {
            if now_ms.saturating_sub(group.window_start) < window {
                return true; // still open
            }
            let Some(last) = group.last.take() else {
                return false; // quiet window: close the group
            };
            let mut summary = last.clone();
            summary.title = format!(
                "{}: {} more in the last {}s",
                group.label, group.extra, limits.aggregate_secs
            );
            summary.body = format!("Latest: {} — {}", last.title, last.body);
            summary.group = None;
            out.push((summary, group.channels.clone()));
            // Keep the group with a fresh window so a steady stream gives one summary per window.
            group.window_start = now_ms;
            group.extra = 0;
            true
        });
        out
    }

    /// Rate limit for one channel. `Some(n)` = may send (`n` were suppressed before it),
    /// `None` = over the limit, dropped.
    pub fn allow(
        &mut self,
        channel: Channel,
        question: bool,
        now_ms: u64,
        limits: &Limits,
    ) -> Option<u32> {
        let sent = self.sent.entry(channel).or_default();
        while sent
            .front()
            .is_some_and(|at| now_ms.saturating_sub(*at) >= 60_000)
        {
            sent.pop_front();
        }
        if !question && sent.len() >= limits.max_per_minute as usize {
            *self.suppressed.entry(channel).or_default() += 1;
            return None;
        }
        sent.push_back(now_ms);
        Some(self.suppressed.remove(&channel).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, Level};

    fn row_failed(row: u32) -> Notification {
        let mut n = Notification::new(
            format!("Run r1: row {row} failed"),
            "NOT_FOUND",
            Level::Error,
        );
        n.group = Some(("run.row_failed:r1".into(), "Run r1 row failures".into()));
        n
    }

    #[test]
    fn a_thousand_row_failures_become_a_few_messages() {
        let limits = Limits::default(); // 60 s windows
        let mut limiter = Limiter::default();
        let mut sent = 0;
        let mut summaries = Vec::new();
        // 1000 failures over 100 s (one per 100 ms), flushing every second.
        for i in 0..1000u64 {
            let now = i * 100;
            if now % 1000 == 0 {
                summaries.extend(limiter.flush(now, &limits));
            }
            if limiter.aggregate(&row_failed(i as u32), &[Channel::Ui], now) {
                sent += 1;
            }
        }
        // After the last event: one more summary when its window ends, then the group closes.
        summaries.extend(limiter.flush(200_000, &limits));
        assert!(limiter.flush(400_000, &limits).is_empty());

        assert_eq!(sent, 1, "only the first failure goes out at once");
        assert_eq!(summaries.len(), 2);
        assert_eq!(
            summaries[0].0.title,
            "Run r1 row failures: 599 more in the last 60s"
        );
        assert_eq!(
            summaries[0].0.body,
            "Latest: Run r1: row 599 failed — NOT_FOUND"
        );
        assert_eq!(
            summaries[1].0.title,
            "Run r1 row failures: 400 more in the last 60s"
        );
        assert_eq!(summaries[1].1, vec![Channel::Ui]);

        // A new burst later starts with an immediate message again.
        assert!(limiter.aggregate(&row_failed(5), &[Channel::Ui], 500_000));
    }

    #[test]
    fn groups_are_separate() {
        let mut limiter = Limiter::default();
        let mut other = row_failed(1);
        other.group = Some(("run.row_failed:r2".into(), "Run r2 row failures".into()));
        assert!(limiter.aggregate(&row_failed(1), &[], 0));
        assert!(limiter.aggregate(&other, &[], 10));
        assert!(!limiter.aggregate(&row_failed(2), &[], 20));
        // Ungrouped notifications are never absorbed.
        let plain = Notification::new("x", "y", Level::Info);
        assert!(limiter.aggregate(&plain, &[], 30));
        assert!(limiter.aggregate(&plain, &[], 40));
    }

    #[test]
    fn duplicates_are_dropped_within_the_window() {
        let limits = Limits::default(); // 10 s
        let mut limiter = Limiter::default();
        let n = Notification::new("Run r1 finished", "ok", Level::Success);
        assert!(limiter.dedup(&n, 0, &limits));
        assert!(!limiter.dedup(&n, 5_000, &limits));
        assert!(limiter.dedup(&n, 10_000, &limits));
        let other = Notification::new("Run r2 finished", "ok", Level::Success);
        assert!(limiter.dedup(&other, 10_001, &limits));
        // Questions are never dropped.
        let mut q = n.clone();
        q.actions.push(Action::new("c0", "Yes"));
        assert!(limiter.dedup(&q, 10_002, &limits));
        assert!(limiter.dedup(&q, 10_003, &limits));
    }

    #[test]
    fn rate_limit_counts_what_it_dropped() {
        let limits = Limits {
            max_per_minute: 3,
            ..Limits::default()
        };
        let mut limiter = Limiter::default();
        for i in 0..3 {
            assert_eq!(limiter.allow(Channel::Telegram, false, i, &limits), Some(0));
        }
        assert_eq!(limiter.allow(Channel::Telegram, false, 10, &limits), None);
        assert_eq!(limiter.allow(Channel::Telegram, false, 11, &limits), None);
        // Other channels have their own budget; questions always pass.
        assert_eq!(limiter.allow(Channel::Ui, false, 12, &limits), Some(0));
        assert_eq!(limiter.allow(Channel::Telegram, true, 13, &limits), Some(2));
        // A minute later there is room again.
        assert_eq!(
            limiter.allow(Channel::Telegram, false, 60_005, &limits),
            Some(0)
        );
    }
}
