//! Per-tab counters of what the gate allowed and blocked (`adblock.stats`).
//!
//! The gate records every decision here, whoever calls it: today the interim CDP `Fetch`
//! interception, later the CEF IO-thread hook. The lock is held for a few map updates only.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use bmcp_core::TabId;
use schemars::JsonSchema;
use serde::Serialize;

use crate::engine::ResourceType;
use crate::gate::{GateAction, GateDecision, Reason};

/// How many distinct blocked hosts we remember per tab.
const MAX_HOSTS: usize = 200;

/// Counters for one tab (or for requests without a tab: service workers, downloads).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct TabStats {
    /// Requests the gate was asked about.
    pub requests: u64,
    /// Requests cancelled (by filter lists or site policy).
    pub blocked: u64,
    /// Requests answered with a replacement body (`$redirect`).
    pub redirected: u64,
    /// Blocked by filter lists.
    pub by_filter: u64,
    /// Blocked by a site policy (js/images/fonts/media/third_party).
    pub by_policy: u64,
    /// Blocked requests by resource type (`script`, `image`, …).
    pub blocked_by_type: BTreeMap<String, u64>,
    /// Blocked requests by host (at most 200 hosts).
    pub blocked_hosts: BTreeMap<String, u64>,
    /// Bytes received by allowed requests, when the caller reports them (CEF hook).
    pub received_bytes: u64,
}

/// Counters of every tab.
#[derive(Debug, Default)]
pub struct Stats {
    tabs: Mutex<HashMap<Option<TabId>, TabStats>>,
}

impl Stats {
    /// Count one decision.
    pub fn record(
        &self,
        tab: Option<TabId>,
        url: &str,
        resource_type: ResourceType,
        decision: &GateDecision,
    ) {
        let mut tabs = self.lock();
        let stats = tabs.entry(tab).or_default();
        stats.requests += 1;
        match decision.action {
            GateAction::Allow => return,
            GateAction::Redirect => stats.redirected += 1,
            GateAction::Block => stats.blocked += 1,
        }
        match decision.reason {
            Some(Reason::Filter) | None => stats.by_filter += 1,
            Some(_) => stats.by_policy += 1,
        }
        let ty = serde_json::to_value(resource_type)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "other".into());
        *stats.blocked_by_type.entry(ty).or_default() += 1;
        if let Some((host, _)) = crate::engine::host_and_domain(url)
            && (stats.blocked_hosts.len() < MAX_HOSTS || stats.blocked_hosts.contains_key(&host))
        {
            *stats.blocked_hosts.entry(host).or_default() += 1;
        }
    }

    /// Count bytes of a finished (allowed) request.
    pub fn add_bytes(&self, tab: Option<TabId>, bytes: u64) {
        self.lock().entry(tab).or_default().received_bytes += bytes;
    }

    /// One tab's counters (zeros if nothing was recorded).
    pub fn tab(&self, tab: TabId) -> TabStats {
        self.lock().get(&Some(tab)).cloned().unwrap_or_default()
    }

    /// Every tab with counters, plus the sum over all of them (including tab-less requests).
    pub fn all(&self) -> (BTreeMap<TabId, TabStats>, TabStats) {
        let tabs = self.lock();
        let mut total = TabStats::default();
        for stats in tabs.values() {
            total.requests += stats.requests;
            total.blocked += stats.blocked;
            total.redirected += stats.redirected;
            total.by_filter += stats.by_filter;
            total.by_policy += stats.by_policy;
            total.received_bytes += stats.received_bytes;
            for (ty, n) in &stats.blocked_by_type {
                *total.blocked_by_type.entry(ty.clone()).or_default() += n;
            }
        }
        let per_tab = tabs
            .iter()
            .filter_map(|(tab, stats)| Some(((*tab)?, stats.clone())))
            .collect();
        (per_tab, total)
    }

    /// Forget one tab's counters (`None` = all tabs).
    pub fn reset(&self, tab: Option<TabId>) {
        let mut tabs = self.lock();
        match tab {
            Some(tab) => {
                tabs.remove(&Some(tab));
            }
            None => tabs.clear(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Option<TabId>, TabStats>> {
        // Counters only: a panic mid-update cannot leave anything dangerous behind.
        self.tabs.lock().unwrap_or_else(|e| e.into_inner())
    }
}
