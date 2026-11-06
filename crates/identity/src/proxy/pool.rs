//! Proxy pools (`data/config/proxies.json`): named lists of proxies with health info and a
//! rotation strategy.
//!
//! * `round_robin` — each pick takes the next healthy proxy;
//! * `random` — any healthy proxy;
//! * `sticky` — the same proxy for the same `run` id (e.g. one batch-run row keeps its IP),
//!   new runs are spread round-robin.
//!
//! Proxies whose last health check failed are skipped; never-checked ones count as healthy.
//!
//! **Leases** (`proxy.lease`) are for mass runs where every row gets its own isolated profile:
//! one proxy per (run id, row). A lease is sticky (the row keeps its proxy across retries
//! and restarts; leases are stored in the file) until it is released or rotated. New leases
//! go to the proxy with the fewest active leases, so a run spreads over the whole pool
//! whatever the pool's strategy is.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// How many run → proxy assignments a sticky pool remembers (oldest are dropped).
const MAX_STICKY: usize = 1000;
/// How many leases a pool keeps (oldest are dropped; runs should release theirs).
const MAX_LEASES: usize = 10_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    #[default]
    RoundRobin,
    Random,
    Sticky,
}

/// One proxy of a pool, with what the last health check found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyEntry {
    /// Full URL including credentials.
    pub url: String,
    /// `None` = never checked.
    #[serde(default)]
    pub healthy: Option<bool>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    /// Unix seconds of the last check.
    #[serde(default)]
    pub checked_at: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ProxyEntry {
    pub fn new(url: String) -> Self {
        ProxyEntry {
            url,
            healthy: None,
            latency_ms: None,
            checked_at: None,
            error: None,
        }
    }

    fn usable(&self) -> bool {
        self.healthy != Some(false)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pool {
    #[serde(default)]
    pub strategy: Strategy,
    #[serde(default)]
    pub proxies: Vec<ProxyEntry>,
    /// Next index for round-robin.
    #[serde(default)]
    pub cursor: usize,
    /// Sticky assignments: run id → proxy URL, in insertion order.
    #[serde(default)]
    pub sticky: Vec<(String, String)>,
    /// Active leases, oldest first.
    #[serde(default)]
    pub leases: Vec<Lease>,
}

/// One proxy handed to one row of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    pub run: String,
    pub row: u64,
    /// Full proxy URL.
    pub url: String,
    /// Unix seconds when it was leased.
    pub since: u64,
}

/// Result of [`Pool::lease`].
#[derive(Debug, Clone, PartialEq)]
pub struct Leased {
    pub url: String,
    /// True when the row already had this proxy.
    pub reused: bool,
    /// Leases on this proxy now (including this one).
    pub load: usize,
}

/// Contents of `proxies.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProxiesFile {
    #[serde(default)]
    pub pools: BTreeMap<String, Pool>,
    /// Profile id → split routing rules (`proxy.rules_set`, see [`super::route`]).
    #[serde(default)]
    pub routes: BTreeMap<String, super::route::RuleSet>,
}

impl Pool {
    /// Add URLs that are not in the pool yet. Returns how many were new.
    pub fn add(&mut self, urls: impl IntoIterator<Item = String>) -> usize {
        let mut added = 0;
        for url in urls {
            if !self.proxies.iter().any(|p| p.url == url) {
                self.proxies.push(ProxyEntry::new(url));
                added += 1;
            }
        }
        added
    }

    /// Remove one proxy (by URL); also forgets sticky runs that used it.
    pub fn remove(&mut self, url: &str) -> bool {
        let before = self.proxies.len();
        self.proxies.retain(|p| p.url != url);
        self.sticky.retain(|(_, u)| u != url);
        self.leases.retain(|l| l.url != url);
        self.proxies.len() != before
    }

    /// The proxy leased to (run, row), if any.
    pub fn leased(&self, run: &str, row: u64) -> Option<&str> {
        self.leases
            .iter()
            .find(|l| l.run == run && l.row == row)
            .map(|l| l.url.as_str())
    }

    /// Lease a proxy to (run, row). Sticky: an existing lease on a usable proxy is returned
    /// as-is. `rotate`: the row gets a different proxy (when the pool has another usable one),
    /// e.g. after a site blocked its IP. `None` when no proxy is usable.
    pub fn lease(&mut self, run: &str, row: u64, rotate: bool, now: u64) -> Option<Leased> {
        let current = self.leased(run, row).map(str::to_string);
        let current_usable = current
            .as_deref()
            .is_some_and(|u| self.proxies.iter().any(|p| p.url == u && p.usable()));
        if !rotate
            && current_usable
            && let Some(url) = current
        {
            let load = self.load(&url);
            return Some(Leased {
                url,
                reused: true,
                load,
            });
        }
        self.leases.retain(|l| !(l.run == run && l.row == row));
        let len = self.proxies.len();
        if len == 0 {
            return None;
        }
        // Usable proxies starting at the cursor, so equally loaded proxies take turns.
        let usable: Vec<usize> = (0..len)
            .map(|step| (self.cursor + step) % len)
            .filter(|&i| self.proxies[i].usable())
            .collect();
        // Rotating avoids the current proxy when there is another one.
        let others: Vec<usize> = usable
            .iter()
            .copied()
            .filter(|&i| Some(&self.proxies[i].url) != current.as_ref())
            .collect();
        let choices = if rotate && !others.is_empty() {
            others
        } else {
            usable
        };
        // Least-loaded first (min_by_key keeps the first of equal ones).
        let index = *choices
            .iter()
            .min_by_key(|&&i| self.load(&self.proxies[i].url))?;
        self.cursor = (index + 1) % len;
        let url = self.proxies[index].url.clone();
        self.leases.push(Lease {
            run: run.to_string(),
            row,
            url: url.clone(),
            since: now,
        });
        if self.leases.len() > MAX_LEASES {
            self.leases.remove(0);
        }
        let load = self.load(&url);
        Some(Leased {
            url,
            reused: false,
            load,
        })
    }

    /// Release the lease of (run, row), or every lease of the run when `row` is `None`.
    /// Returns how many were released.
    pub fn release(&mut self, run: &str, row: Option<u64>) -> usize {
        let before = self.leases.len();
        self.leases
            .retain(|l| !(l.run == run && row.is_none_or(|r| r == l.row)));
        before - self.leases.len()
    }

    /// Active leases on one proxy.
    pub fn load(&self, url: &str) -> usize {
        self.leases.iter().filter(|l| l.url == url).count()
    }

    /// Pick the proxy URL for the next connection (see the module docs). `None` when no proxy
    /// is usable.
    pub fn pick(&mut self, run: Option<&str>, rng: &mut Rng) -> Option<String> {
        let usable: Vec<usize> = (0..self.proxies.len())
            .filter(|&i| self.proxies[i].usable())
            .collect();
        if usable.is_empty() {
            return None;
        }
        match (self.strategy, run) {
            (Strategy::Random, _) => Some(self.proxies[*rng.pick(&usable)].url.clone()),
            (Strategy::Sticky, Some(run)) => {
                if let Some((_, url)) = self.sticky.iter().find(|(r, _)| r == run)
                    && self.proxies.iter().any(|p| &p.url == url && p.usable())
                {
                    return Some(url.clone());
                }
                let url = self.next_round_robin(&usable);
                self.sticky.retain(|(r, _)| r != run);
                self.sticky.push((run.to_string(), url.clone()));
                if self.sticky.len() > MAX_STICKY {
                    self.sticky.remove(0);
                }
                Some(url)
            }
            // Round-robin, and sticky without a run id.
            _ => Some(self.next_round_robin(&usable)),
        }
    }

    fn next_round_robin(&mut self, usable: &[usize]) -> String {
        // First usable index at or after the cursor, wrapping around.
        let len = self.proxies.len();
        let index = (0..len)
            .map(|step| (self.cursor + step) % len)
            .find(|i| usable.contains(i))
            .unwrap_or(usable[0]);
        self.cursor = (index + 1) % len;
        self.proxies[index].url.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(strategy: Strategy, n: usize) -> Pool {
        let mut p = Pool {
            strategy,
            ..Pool::default()
        };
        p.add((0..n).map(|i| format!("http://p{i}:80")));
        p
    }

    #[test]
    fn round_robin_skips_unhealthy() {
        let mut p = pool(Strategy::RoundRobin, 3);
        let mut rng = Rng::new(1);
        let picks: Vec<String> = (0..4).map(|_| p.pick(None, &mut rng).unwrap()).collect();
        assert_eq!(
            picks,
            [
                "http://p0:80",
                "http://p1:80",
                "http://p2:80",
                "http://p0:80"
            ]
        );
        p.proxies[1].healthy = Some(false);
        let picks: Vec<String> = (0..3).map(|_| p.pick(None, &mut rng).unwrap()).collect();
        assert_eq!(picks, ["http://p2:80", "http://p0:80", "http://p2:80"]);
        for e in &mut p.proxies {
            e.healthy = Some(false);
        }
        assert_eq!(p.pick(None, &mut rng), None);
    }

    #[test]
    fn random_is_deterministic_per_seed_and_covers_the_pool() {
        let mut a = pool(Strategy::Random, 4);
        let mut b = pool(Strategy::Random, 4);
        let pa: Vec<_> = (0..20)
            .map(|_| a.pick(None, &mut Rng::new(9)).unwrap())
            .collect();
        let pb: Vec<_> = (0..20)
            .map(|_| b.pick(None, &mut Rng::new(9)).unwrap())
            .collect();
        assert_eq!(pa, pb);
        let mut rng = Rng::new(3);
        let seen: std::collections::HashSet<_> =
            (0..200).map(|_| a.pick(None, &mut rng).unwrap()).collect();
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn sticky_keeps_a_run_on_one_proxy() {
        let mut p = pool(Strategy::Sticky, 3);
        let mut rng = Rng::new(1);
        let r1 = p.pick(Some("run-1"), &mut rng).unwrap();
        let r2 = p.pick(Some("run-2"), &mut rng).unwrap();
        assert_ne!(r1, r2);
        for _ in 0..5 {
            assert_eq!(p.pick(Some("run-1"), &mut rng).unwrap(), r1);
        }
        // Its proxy goes bad: the run moves, and stays on the new one.
        p.proxies.iter_mut().find(|e| e.url == r1).unwrap().healthy = Some(false);
        let moved = p.pick(Some("run-1"), &mut rng).unwrap();
        assert_ne!(moved, r1);
        assert_eq!(p.pick(Some("run-1"), &mut rng).unwrap(), moved);
        // Removing a proxy forgets its runs.
        assert!(p.remove(&moved));
        assert!(!p.sticky.iter().any(|(_, u)| *u == moved));
    }

    #[test]
    fn leases_are_sticky_spread_and_rotatable() {
        let mut p = pool(Strategy::RoundRobin, 3);
        // Three rows spread over three proxies.
        let a = p.lease("run", 0, false, 1).unwrap();
        let b = p.lease("run", 1, false, 1).unwrap();
        let c = p.lease("run", 2, false, 1).unwrap();
        let mut urls = vec![a.url.clone(), b.url.clone(), c.url.clone()];
        urls.sort();
        urls.dedup();
        assert_eq!(urls.len(), 3, "least-loaded proxy first");
        assert!(!a.reused && a.load == 1);
        // Sticky: the same row keeps its proxy.
        let again = p.lease("run", 0, false, 2).unwrap();
        assert_eq!((again.url.as_str(), again.reused), (a.url.as_str(), true));
        assert_eq!(p.leased("run", 0), Some(a.url.as_str()));
        // A fourth row shares a proxy (load 2).
        assert_eq!(p.lease("run", 3, false, 1).unwrap().load, 2);
        // Rotate: a different proxy.
        let rotated = p.lease("run", 0, true, 3).unwrap();
        assert_ne!(rotated.url, a.url);
        assert_eq!(p.leased("run", 0), Some(rotated.url.as_str()));
        // Its proxy goes bad: the row moves on the next (sticky) lease.
        let bad = rotated.url.clone();
        p.proxies.iter_mut().find(|e| e.url == bad).unwrap().healthy = Some(false);
        let moved = p.lease("run", 0, false, 4).unwrap();
        assert!(moved.url != bad && !moved.reused);
        // Release one row, then the whole run.
        assert_eq!(p.release("run", Some(0)), 1);
        assert_eq!(p.leased("run", 0), None);
        assert_eq!(p.release("other", None), 0);
        assert_eq!(p.release("run", None), 3);
        assert!(p.leases.is_empty());
        // Removing a proxy drops its leases; no usable proxy gives None.
        let l = p.lease("r2", 0, false, 5).unwrap();
        assert!(p.remove(&l.url));
        assert_eq!(p.leased("r2", 0), None);
        for e in &mut p.proxies {
            e.healthy = Some(false);
        }
        assert_eq!(p.lease("r2", 0, false, 6), None);
        // Rotating in a one-proxy pool keeps the only proxy.
        let mut one = pool(Strategy::Sticky, 1);
        let first = one.lease("r", 0, false, 1).unwrap();
        assert_eq!(one.lease("r", 0, true, 2).unwrap().url, first.url);
        assert_eq!(Pool::default().lease("r", 0, false, 1), None);
    }

    #[test]
    fn add_ignores_duplicates() {
        let mut p = pool(Strategy::RoundRobin, 2);
        assert_eq!(
            p.add(["http://p1:80".to_string(), "http://p9:80".to_string()]),
            1
        );
        assert_eq!(p.proxies.len(), 3);
    }
}
