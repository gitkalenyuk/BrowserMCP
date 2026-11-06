//! Per-tab traffic state that lives between command calls.
//!
//! [`NetState`] (one per browser) sits in the registry's `Extensions` map, like
//! `PerceptionState` in `bmcp-browser`. It holds one [`TabNet`] per tab with the captured
//! requests, the rules/breakpoints, the paused requests and the background task that reads the
//! tab's CDP events ([`crate::listener`]).
//!
//! Locking: the data is behind a plain `std::sync::Mutex` that is only held for a few lines and
//! never across an `.await`. Changes to the CDP setup (hold/release `Network`, update the Fetch
//! patterns) also take the async `config` lock, so two commands never interleave those calls.
//!
//! The CDP session is shared with other subsystems (record, adblock), so net never sends
//! `Network.enable/disable` or `Fetch.enable/disable` itself: it holds `Network` through the
//! domain refcount and registers its rules as the "net" client of the tab's Fetch coordinator
//! (both in `bmcp-cdpshare`).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use bmcp_cdpshare::{CdpShare, FetchClient, ORDER_NET};
use bmcp_core::{BrowserBackend, CmdError, Ctx, EventBus, TabId};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::rules::{Rule, Stage};
use crate::store::{TrafficStore, cut};

/// All tabs' traffic state. One instance per browser.
#[derive(Default)]
pub struct NetState {
    tabs: Mutex<HashMap<TabId, Arc<TabNet>>>,
    /// Shared counter for rule (`r1`), breakpoint (`b2`) and pause (`p3`) ids, so an id alone
    /// says which tab it belongs to.
    next_id: AtomicU64,
    /// Where `net.har_export` writes files (default: `<data>/har`).
    har_dir: Mutex<Option<PathBuf>>,
}

impl NetState {
    /// The shared instance from the call's context (created on first use).
    pub fn from_ctx(ctx: &Ctx) -> Arc<NetState> {
        ctx.extensions.get_or_insert_with(NetState::default)
    }

    /// The state of one tab, created on first use.
    pub fn tab(&self, tab: TabId) -> Arc<TabNet> {
        lock(&self.tabs)
            .entry(tab)
            .or_insert_with(|| Arc::new(TabNet::new(tab)))
            .clone()
    }

    /// The state of one tab if anything was ever done there.
    pub fn existing(&self, tab: TabId) -> Option<Arc<TabNet>> {
        lock(&self.tabs).get(&tab).cloned()
    }

    /// Every tab with state, by tab id.
    pub fn all(&self) -> Vec<Arc<TabNet>> {
        let mut tabs: Vec<_> = lock(&self.tabs).values().cloned().collect();
        tabs.sort_by_key(|t| t.tab);
        tabs
    }

    /// A new id like `r7` (`prefix` = "r", "b" or "p").
    pub fn new_id(&self, prefix: &str) -> String {
        format!(
            "{prefix}{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        )
    }

    /// Set the folder for HAR files (the app does this from `--data-dir`).
    pub fn set_har_dir(&self, dir: PathBuf) {
        *lock(&self.har_dir) = Some(dir);
    }

    pub fn har_dir(&self) -> Option<PathBuf> {
        lock(&self.har_dir).clone()
    }
}

/// One tab's traffic state.
pub struct TabNet {
    pub tab: TabId,
    inner: Mutex<TabInner>,
    /// Held while changing the CDP setup of this tab (see the module docs).
    pub config: tokio::sync::Mutex<()>,
}

/// The data of a [`TabNet`]; get it with [`TabNet::lock`].
#[derive(Default)]
pub struct TabInner {
    /// `Network` domain enabled by `net.capture on`.
    pub capture: bool,
    /// Also download every response body when it finishes (instead of on demand).
    pub bodies: bool,
    pub store: TrafficStore,
    /// Rules and breakpoints in the order they were added. They are checked in that order: a
    /// block/mock before the first matching breakpoint answers the request (no pause);
    /// otherwise the first matching breakpoint pauses it (see `intercept::on_paused`).
    pub rules: Vec<Rule>,
    /// Requests held by a breakpoint, by pause id.
    pub paused: BTreeMap<String, Paused>,
    /// The background task reading this tab's `Network.*` events (while capturing).
    listener: Option<JoinHandle<()>>,
}

impl TabNet {
    fn new(tab: TabId) -> Self {
        TabNet {
            tab,
            inner: Mutex::new(TabInner::default()),
            config: tokio::sync::Mutex::new(()),
        }
    }

    /// Lock the data. Keep the guard only for a few lines, never across an `.await`.
    pub fn lock(&self) -> MutexGuard<'_, TabInner> {
        lock(&self.inner)
    }
}

impl TabInner {
    pub fn listener_running(&self) -> bool {
        self.listener.as_ref().is_some_and(|h| !h.is_finished())
    }

    pub fn set_listener(&mut self, handle: JoinHandle<()>) {
        if let Some(old) = self.listener.replace(handle) {
            old.abort();
        }
    }

    /// Stop the listener when capture is off (rules and breakpoints do not need it: their
    /// paused requests come from the Fetch coordinator).
    pub fn stop_listener_if_idle(&mut self) {
        if !self.capture
            && let Some(handle) = self.listener.take()
        {
            handle.abort();
        }
    }

    /// `Fetch.enable` patterns the current rules need (deduplicated, in rule order).
    pub fn wanted_patterns(&self) -> Vec<Value> {
        let mut patterns: Vec<Value> = Vec::new();
        for rule in &self.rules {
            let p = rule.matcher.pattern();
            if !patterns.contains(&p) {
                patterns.push(p);
            }
        }
        patterns
    }
}

/// A request held by a breakpoint.
#[derive(Debug, Clone)]
pub struct Paused {
    pub pause_id: String,
    pub tab: TabId,
    /// The breakpoint that caught it.
    pub rule_id: String,
    /// What the matching action rules would do with it: applied when it is released without
    /// edits (`net.resume {}`, the 60 s timeout, removing the breakpoint).
    pub actions: Vec<crate::rules::Action>,
    /// `Fetch` request id (not the `Network` one).
    pub request_id: String,
    pub stage: Stage,
    /// The original `Fetch.requestPaused` parameters.
    pub params: Value,
    /// When it is continued automatically.
    pub deadline: tokio::time::Instant,
}

impl Paused {
    pub fn url(&self) -> &str {
        self.params["request"]["url"].as_str().unwrap_or_default()
    }

    pub fn method(&self) -> &str {
        self.params["request"]["method"]
            .as_str()
            .unwrap_or_default()
    }

    /// Payload of the `net.paused` event: what an agent needs to decide on `net.resume`.
    pub fn info(&self) -> Value {
        let r = &self.params["request"];
        let mut info = json!({
            "pause_id": self.pause_id,
            "tab": self.tab,
            "rule": self.rule_id,
            "stage": self.stage,
            "method": self.method(),
            "url": self.url(),
            "resource_type": self.params["resourceType"].as_str().unwrap_or("other").to_ascii_lowercase(),
            "headers": r["headers"],
        });
        if let Some(post) = r["postData"].as_str() {
            info["post_data"] = json!(cut(post, 2048));
        }
        if !self.actions.is_empty() {
            let pending: Vec<String> = self.actions.iter().map(|a| a.short()).collect();
            info["pending_actions"] = json!(pending);
        }
        if self.stage == Stage::Response {
            info["status"] = self.params["responseStatusCode"].clone();
            info["response_headers"] = self.params["responseHeaders"].clone();
            if let Some(err) = self.params["responseErrorReason"].as_str() {
                info["error"] = json!(err);
            }
        }
        info
    }

    /// `p3 b2 tab 1 request POST https://… (42s left)`.
    pub fn line(&self) -> String {
        let left = self
            .deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_secs();
        let status = match self.params["responseStatusCode"].as_u64() {
            Some(s) if self.stage == Stage::Response => format!(" {s}"),
            _ => String::new(),
        };
        format!(
            "{} {} tab {} {} {}{} {} ({left}s left)",
            self.pause_id,
            self.rule_id,
            self.tab,
            self.stage.as_str(),
            self.method(),
            status,
            crate::format::short_url(self.url(), 120),
        )
    }
}

/// What background work (the listener, auto-continue timers) needs to act on one tab.
#[derive(Clone)]
pub struct Env {
    pub tab: TabId,
    pub tn: Arc<TabNet>,
    pub net: Arc<NetState>,
    pub backend: Arc<dyn BrowserBackend>,
    pub events: EventBus,
    /// The tab's shared CDP state (domain refcount, Fetch coordinator).
    pub share: Arc<CdpShare>,
}

impl Env {
    pub fn new(ctx: &Ctx, tab: TabId) -> Env {
        let net = NetState::from_ctx(ctx);
        Env::for_tab(ctx, net.tab(tab))
    }

    /// The environment of an existing tab state.
    pub fn for_tab(ctx: &Ctx, tn: Arc<TabNet>) -> Env {
        Env {
            tab: tn.tab,
            tn,
            net: NetState::from_ctx(ctx),
            backend: ctx.backend.clone(),
            events: ctx.events.clone(),
            share: CdpShare::from_ctx(ctx),
        }
    }

    /// One CDP call on this tab.
    pub async fn cdp(&self, method: &str, params: Value) -> Result<Value, CmdError> {
        Ok(self
            .backend
            .cdp(self.tab, method.to_string(), params)
            .await?)
    }

    /// Start the listener task if it is not running. Subscribes *before* any domain is
    /// enabled, so no event is missed.
    pub async fn ensure_listener(&self) -> Result<(), CmdError> {
        if self.tn.lock().listener_running() {
            return Ok(());
        }
        let events = self.backend.cdp_events(self.tab).await?;
        let handle = crate::listener::spawn(self.clone(), events);
        self.tn.lock().set_listener(handle);
        Ok(())
    }

    /// Give the tab's Fetch coordinator the patterns of the current rules (as client "net"),
    /// or unregister when there are none. The coordinator enables Fetch with the union of all
    /// clients' patterns and disables it only when no client is left (interception slows every
    /// request down, so only while needed). Call with the tab's `config` lock held.
    pub async fn sync_fetch(&self) -> Result<(), CmdError> {
        let wanted = self.tn.lock().wanted_patterns();
        if wanted.is_empty() {
            self.share
                .clear_fetch(&self.backend, self.tab, crate::intercept::CLIENT)
                .await?;
        } else {
            let client = FetchClient {
                name: crate::intercept::CLIENT.to_string(),
                order: ORDER_NET,
                patterns: wanted,
                handler: Arc::new(crate::intercept::NetFetch { env: self.clone() }),
            };
            self.share
                .set_fetch(&self.backend, self.tab, client)
                .await?;
        }
        Ok(())
    }
}

/// Lock a std mutex; a poisoned lock (a panic elsewhere) still holds usable data here.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
