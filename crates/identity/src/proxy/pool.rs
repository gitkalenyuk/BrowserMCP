//! Proxy pools (`data/config/proxies.json`): named lists of proxies with health info and a
//! rotation strategy.
//!
//! * `round_robin` — each pick takes the next healthy proxy;
//! * `random` — any healthy proxy;
//! * `sticky` — the same proxy for the same `run` id (e.g. one batch-run row keeps its IP),
//!   new runs are spread round-robin.
//!
//! Proxies whose last health check failed are skipped; never-checked ones count as healthy.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// How many run → proxy assignments a sticky pool remembers (oldest are dropped).
const MAX_STICKY: usize = 1000;

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
}

/// Contents of `proxies.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProxiesFile {
    #[serde(default)]
    pub pools: BTreeMap<String, Pool>,
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
        self.proxies.len() != before
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
    fn add_ignores_duplicates() {
        let mut p = pool(Strategy::RoundRobin, 2);
        assert_eq!(
            p.add(["http://p1:80".to_string(), "http://p9:80".to_string()]),
            1
        );
        assert_eq!(p.proxies.len(), 3);
    }
}
