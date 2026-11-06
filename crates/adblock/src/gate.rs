//! The [`ResourceGate`]: one decision per network request — allow, block or redirect —
//! from the filter lists, the adblock allowlist and the per-site simplification policy.
//!
//! The gate is CEF-agnostic. Callers:
//! * now: the interim CDP `Fetch` interception ([`crate::interim`]);
//! * later: the CEF crate, from `CefResourceRequestHandler::OnBeforeResourceLoad` on the IO
//!   thread (see `crates/adblock/README.md` for the exact hook).
//!
//! Threading: [`Gate`] is `Send + Sync` and shared as `Arc`. Its configuration is one immutable
//! [`GateState`] snapshot that is swapped on every change. The `adblock::Engine` itself is
//! `!Send`, so it is not part of the snapshot: the snapshot only holds the serialized engine
//! bytes, and each thread builds its own [`RequestFilter`] from them on first use and keeps it
//! in a `thread_local!` together with the snapshot it belongs to. The fast path of a request
//! is therefore one atomic load (is my thread's snapshot still current?) and no lock.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use bmcp_core::TabId;
use schemars::JsonSchema;
use serde::Serialize;

use crate::engine::{CosmeticResources, Decision, RequestFilter, ResourceType, host_and_domain};
use crate::policy::{AdblockSettings, ResolvedPolicy, SimplifyPolicies};
use crate::stats::Stats;

/// One request as the browser sees it.
#[derive(Debug, Clone, Copy)]
pub struct RequestInfo<'a> {
    /// The tab that made the request (`None`: service worker, download, browser itself).
    pub tab: Option<TabId>,
    /// The requested URL.
    pub url: &'a str,
    /// URL of the page that made the request (top-level document); empty = unknown. For a
    /// main-frame navigation it is ignored (the page is the request itself).
    pub source_url: &'a str,
    pub resource_type: ResourceType,
}

/// What to do with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateAction {
    Allow,
    /// Cancel it (CEF: `RV_CANCEL`; CDP: `Fetch.failRequest` with `BlockedByClient`).
    Block,
    /// Answer with [`GateDecision::redirect`] (a `data:` URL) instead of loading it.
    Redirect,
}

/// Why a request was blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A filter-list rule.
    Filter,
    /// Site policy `js: false`.
    Js,
    /// Site policy `images: false`.
    Images,
    /// Site policy `fonts: false`.
    Fonts,
    /// Site policy `media: false`.
    Media,
    /// Site policy `third_party: false`.
    ThirdParty,
}

/// The gate's answer for one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct GateDecision {
    pub action: GateAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// Replacement body as a `data:` URL when `action` is `redirect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    /// An `$important` filter matched.
    pub important: bool,
    /// The matching filter rule, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

impl GateDecision {
    pub fn allow() -> Self {
        GateDecision {
            action: GateAction::Allow,
            reason: None,
            redirect: None,
            important: false,
            rule: None,
        }
    }

    fn block(reason: Reason) -> Self {
        GateDecision {
            action: GateAction::Block,
            reason: Some(reason),
            ..Self::allow()
        }
    }

    pub fn is_allowed(&self) -> bool {
        self.action == GateAction::Allow
    }
}

/// The integration point for the browser's network stack. Implementations must be cheap and
/// must never block: `on_request` runs for every request, on CEF's IO thread.
pub trait ResourceGate: Send + Sync + 'static {
    /// Decide about one request (and count it in the per-tab stats).
    fn on_request(&self, req: &RequestInfo<'_>) -> GateDecision;

    /// An allowed request finished with `received_bytes` of body (for the stats).
    fn on_complete(&self, _tab: Option<TabId>, _received_bytes: u64) {}

    /// The site policy's content settings for a page about to load `page_url`: `(javascript
    /// allowed, images allowed)`. The CEF crate turns them into `SetContentSetting` calls
    /// before the navigation commits. `None` = no opinion (leave the defaults).
    fn content_settings(&self, _page_url: &str) -> Option<(bool, bool)> {
        None
    }
}

/// The engine in its shareable (serialized) form.
#[derive(Clone)]
pub struct EngineBytes {
    /// Unique per compiled engine, so threads know when to rebuild their copy.
    pub id: u64,
    pub bytes: Arc<Vec<u8>>,
}

/// One immutable configuration snapshot of the gate.
#[derive(Clone, Default)]
pub struct GateState {
    /// Unique per snapshot (see [`Gate`]).
    pub generation: u64,
    /// `None` until filter lists are loaded: only site policies apply then.
    pub engine: Option<EngineBytes>,
    pub settings: AdblockSettings,
    pub policies: SimplifyPolicies,
}

/// Everything the gate considered for one request (for `adblock.check`).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Evaluation {
    pub decision: GateDecision,
    /// The filter lists' own verdict (`None`: lists not consulted — adblock off, allowlisted
    /// site, no engine loaded, or the request was a main-frame navigation).
    pub filter: Option<Decision>,
    /// The page's effective site policy.
    pub policy: ResolvedPolicy,
    /// Host of the page that made the request.
    pub page_host: Option<String>,
    /// The request goes to another registrable domain than the page.
    pub third_party: bool,
    /// Filter lists are active for this page.
    pub adblock_active: bool,
}

/// Source of unique snapshot/engine/gate ids (one counter for all, so they never collide).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// What one thread remembers about one gate.
struct ThreadView {
    gate: u64,
    state: Arc<GateState>,
    /// The thread's own engine, built from `state.engine` (`(engine id, filter)`).
    filter: Option<(u64, Rc<RequestFilter>)>,
}

thread_local! {
    // One slot per thread: production has one gate; tests that alternate gates just rebuild.
    static VIEW: RefCell<Option<ThreadView>> = const { RefCell::new(None) };
}

/// The shared gate. See the module docs.
pub struct Gate {
    id: u64,
    /// Generation of the current `state`, readable without the lock.
    generation: AtomicU64,
    state: RwLock<Arc<GateState>>,
    pub stats: Stats,
    /// True once the browser's network stack asks this gate for every request (the CEF
    /// `OnBeforeResourceLoad` hook). `adblock.apply` then skips its CDP `Fetch` interception.
    native: AtomicBool,
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new(AdblockSettings::default(), SimplifyPolicies::default())
    }
}

impl Gate {
    pub fn new(settings: AdblockSettings, policies: SimplifyPolicies) -> Self {
        let state = GateState {
            generation: next_id(),
            engine: None,
            settings,
            policies,
        };
        Gate {
            id: next_id(),
            generation: AtomicU64::new(state.generation),
            state: RwLock::new(Arc::new(state)),
            stats: Stats::default(),
            native: AtomicBool::new(false),
        }
    }

    /// Mark the gate as enforced by the browser itself (see [`Gate::is_native`]).
    pub fn set_native(&self, on: bool) {
        self.native.store(on, Ordering::Relaxed);
    }

    /// Does the browser's network stack already ask this gate for every request?
    pub fn is_native(&self) -> bool {
        self.native.load(Ordering::Relaxed)
    }

    /// The current configuration snapshot.
    pub fn state(&self) -> Arc<GateState> {
        self.state.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Change the configuration: `change` edits a copy, which then replaces the snapshot.
    pub fn update(&self, change: impl FnOnce(&mut GateState)) {
        let mut guard = self.state.write().unwrap_or_else(|e| e.into_inner());
        let mut next = (**guard).clone();
        change(&mut next);
        next.generation = next_id();
        self.generation.store(next.generation, Ordering::Release);
        *guard = Arc::new(next);
    }

    /// Use a new compiled engine (bytes from [`RequestFilter::serialize`]).
    pub fn set_engine(&self, bytes: Vec<u8>) {
        let engine = EngineBytes {
            id: next_id(),
            bytes: Arc::new(bytes),
        };
        self.update(|state| state.engine = Some(engine));
    }

    pub fn has_engine(&self) -> bool {
        self.state().engine.is_some()
    }

    /// This thread's view: the current snapshot and this thread's engine for it.
    fn view(&self) -> (Arc<GateState>, Option<Rc<RequestFilter>>) {
        let generation = self.generation.load(Ordering::Acquire);
        VIEW.with(|cell| {
            let mut view = cell.borrow_mut();
            let current =
                matches!(&*view, Some(v) if v.gate == self.id && v.state.generation == generation);
            if !current {
                let state = self.state();
                // Keep this thread's engine if only the settings/policies changed.
                let old = view
                    .take()
                    .filter(|v| v.gate == self.id)
                    .and_then(|v| v.filter);
                let filter = match (&state.engine, old) {
                    (None, _) => None,
                    (Some(engine), Some((id, filter))) if id == engine.id => Some((id, filter)),
                    (Some(engine), _) => build_thread_filter(engine),
                };
                *view = Some(ThreadView {
                    gate: self.id,
                    state,
                    filter,
                });
            }
            let view = view.as_ref().expect("view was just set");
            (
                view.state.clone(),
                view.filter.as_ref().map(|(_, f)| f.clone()),
            )
        })
    }

    /// Run `f` with this thread's engine (`None` when no lists are loaded).
    pub fn with_filter<R>(&self, f: impl FnOnce(&RequestFilter) -> R) -> Option<R> {
        let (_, filter) = self.view();
        filter.map(|filter| f(&filter))
    }

    /// Decide about a request without counting it.
    pub fn evaluate(&self, req: &RequestInfo<'_>) -> Evaluation {
        let (state, filter) = self.view();
        let is_main_frame = req.resource_type == ResourceType::MainFrame;
        let page_url = if is_main_frame || req.source_url.is_empty() {
            req.url
        } else {
            req.source_url
        };
        let page = host_and_domain(page_url);
        let target = host_and_domain(req.url);
        let third_party = match (&page, &target) {
            (Some((_, page_domain)), Some((_, target_domain))) => page_domain != target_domain,
            _ => false,
        };
        let policy = page
            .as_ref()
            .map(|(host, _)| state.policies.effective(host).0.resolve())
            .unwrap_or_default();
        let page_host = page.as_ref().map(|(host, _)| host.clone());
        let adblock_active = filter.is_some()
            && state.settings.enabled
            && !page_host
                .as_deref()
                .is_some_and(|h| state.settings.is_allowlisted(h));

        let mut evaluation = Evaluation {
            decision: GateDecision::allow(),
            filter: None,
            policy,
            page_host,
            third_party,
            adblock_active,
        };
        // Navigations are never blocked here (that would leave an error page instead).
        if is_main_frame || !crate::engine::is_http(req.url) {
            return evaluation;
        }
        if let Some(reason) = policy_block(&policy, req.resource_type, third_party) {
            evaluation.decision = GateDecision::block(reason);
            return evaluation;
        }
        if let (true, Some(filter)) = (adblock_active, filter) {
            let verdict = filter.check(req.url, page_url, req.resource_type);
            if verdict.block {
                evaluation.decision = GateDecision {
                    action: if verdict.redirect.is_some() {
                        GateAction::Redirect
                    } else {
                        GateAction::Block
                    },
                    reason: Some(Reason::Filter),
                    redirect: verdict.redirect.clone(),
                    important: verdict.important,
                    rule: verdict.rule.clone(),
                };
            }
            evaluation.filter = Some(verdict);
        }
        evaluation
    }

    /// Cosmetic resources for a page, or `None` when filter lists do not apply to it
    /// (adblock off, allowlisted, no lists loaded, not an http(s) page).
    pub fn cosmetic(&self, page_url: &str) -> Option<CosmeticResources> {
        let (state, filter) = self.view();
        let (host, _) = host_and_domain(page_url)?;
        if !state.settings.enabled || state.settings.is_allowlisted(&host) {
            return None;
        }
        Some(filter?.cosmetic(page_url))
    }
}

impl ResourceGate for Gate {
    fn on_request(&self, req: &RequestInfo<'_>) -> GateDecision {
        let decision = self.evaluate(req).decision;
        self.stats
            .record(req.tab, req.url, req.resource_type, &decision);
        decision
    }

    fn on_complete(&self, tab: Option<TabId>, received_bytes: u64) {
        self.stats.add_bytes(tab, received_bytes);
    }

    fn content_settings(&self, page_url: &str) -> Option<(bool, bool)> {
        let (host, _) = host_and_domain(page_url)?;
        let policy = self.state().policies.effective(&host).0.resolve();
        Some((policy.js, policy.images))
    }
}

/// Which policy rule (if any) forbids this request.
fn policy_block(policy: &ResolvedPolicy, ty: ResourceType, third_party: bool) -> Option<Reason> {
    let by_type = match ty {
        ResourceType::Script | ResourceType::Worker if !policy.js => Some(Reason::Js),
        ResourceType::Image | ResourceType::Favicon if !policy.images => Some(Reason::Images),
        ResourceType::Font if !policy.fonts => Some(Reason::Fonts),
        ResourceType::Media if !policy.media => Some(Reason::Media),
        _ => None,
    };
    by_type.or(if third_party && !policy.third_party {
        Some(Reason::ThirdParty)
    } else {
        None
    })
}

/// Deserialize the shared engine bytes into this thread's own engine.
fn build_thread_filter(engine: &EngineBytes) -> Option<(u64, Rc<RequestFilter>)> {
    let started = Instant::now();
    match RequestFilter::from_serialized(&engine.bytes) {
        Ok(filter) => {
            tracing::debug!(
                thread = ?std::thread::current().name(),
                ms = started.elapsed().as_millis() as u64,
                "adblock engine loaded on this thread"
            );
            Some((engine.id, Rc::new(filter)))
        }
        Err(err) => {
            tracing::warn!(%err, "adblock engine unusable on this thread");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::SitePolicy;

    const RULES: &str = "||ads.example^\n||cdn.test/lib.js$script\nnews.test##.sponsored\n";

    fn gate() -> Gate {
        let gate = Gate::default();
        gate.set_engine(RequestFilter::compile(&[RULES]));
        gate
    }

    fn req<'a>(url: &'a str, page: &'a str, ty: ResourceType) -> RequestInfo<'a> {
        RequestInfo {
            tab: Some(TabId(1)),
            url,
            source_url: page,
            resource_type: ty,
        }
    }

    #[test]
    fn filter_blocks_and_stats_count() {
        let gate = gate();
        let page = "https://news.test/";
        let d = gate.on_request(&req("https://ads.example/a.js", page, ResourceType::Script));
        assert_eq!(d.action, GateAction::Block);
        assert_eq!(d.reason, Some(Reason::Filter));
        assert!(
            gate.on_request(&req("https://news.test/app.js", page, ResourceType::Script))
                .is_allowed()
        );
        // Navigations are never blocked.
        assert!(
            gate.on_request(&req("https://ads.example/", "", ResourceType::MainFrame))
                .is_allowed()
        );
        let stats = gate.stats.tab(TabId(1));
        assert_eq!((stats.requests, stats.blocked, stats.by_filter), (3, 1, 1));
        assert_eq!(stats.blocked_hosts["ads.example"], 1);
        assert_eq!(stats.blocked_by_type["script"], 1);
    }

    #[test]
    fn allowlist_and_global_switch() {
        let gate = gate();
        let r = req(
            "https://ads.example/a.js",
            "https://www.news.test/",
            ResourceType::Script,
        );
        gate.update(|s| {
            s.settings.allowlist.insert("news.test".into());
        });
        assert!(gate.on_request(&r).is_allowed());
        assert!(gate.cosmetic("https://news.test/").is_none());
        gate.update(|s| {
            s.settings.allowlist.clear();
            s.settings.enabled = false;
        });
        assert!(gate.on_request(&r).is_allowed());
        gate.update(|s| s.settings.enabled = true);
        assert!(!gate.on_request(&r).is_allowed());
    }

    #[test]
    fn site_policy_blocks_by_type_and_third_party() {
        let gate = gate();
        gate.update(|s| {
            s.policies.merge(
                "news.test",
                SitePolicy {
                    images: Some(false),
                    third_party: Some(false),
                    ..SitePolicy::default()
                },
            );
        });
        let page = "https://news.test/";
        let d = gate.on_request(&req("https://news.test/a.png", page, ResourceType::Image));
        assert_eq!(d.reason, Some(Reason::Images));
        let d = gate.on_request(&req(
            "https://fonts.other.test/f.woff2",
            page,
            ResourceType::Font,
        ));
        assert_eq!(d.reason, Some(Reason::ThirdParty));
        // Same registrable domain = first party.
        assert!(
            gate.on_request(&req(
                "https://static.news.test/x.js",
                page,
                ResourceType::Script
            ))
            .is_allowed()
        );
        // Policies apply even with adblock switched off.
        gate.update(|s| s.settings.enabled = false);
        let e = gate.evaluate(&req(
            "https://cdn.other.test/x.js",
            page,
            ResourceType::Script,
        ));
        assert_eq!(e.decision.reason, Some(Reason::ThirdParty));
        assert!(e.third_party && !e.adblock_active);
        assert_eq!(gate.stats.tab(TabId(1)).by_policy, 2);
    }

    #[test]
    fn no_engine_means_policies_only() {
        let gate = Gate::default();
        let e = gate.evaluate(&req(
            "https://ads.example/a.js",
            "https://news.test/",
            ResourceType::Script,
        ));
        assert!(e.decision.is_allowed() && e.filter.is_none() && !e.adblock_active);
    }

    #[test]
    fn engine_swaps_reach_every_thread() {
        let gate = Arc::new(gate());
        let r = || {
            req(
                "https://other-ads.test/x.js",
                "https://news.test/",
                ResourceType::Script,
            )
        };
        // Build the per-thread engine on this thread first.
        assert!(gate.evaluate(&r()).decision.is_allowed());
        gate.set_engine(RequestFilter::compile(&["||other-ads.test^"]));
        assert!(!gate.evaluate(&r()).decision.is_allowed());
        // Another thread builds its own copy from the shared bytes.
        let shared = gate.clone();
        let blocked = std::thread::spawn(move || !shared.evaluate(&r()).decision.is_allowed())
            .join()
            .unwrap();
        assert!(blocked);
    }
}
