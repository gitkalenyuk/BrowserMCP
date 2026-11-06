//! Shared use of one tab's CDP session by several subsystems.
//!
//! Every subsystem (`bmcp-net`, `bmcp-record`, `bmcp-adblock`, …) talks to a tab through the
//! same CDP session (`BrowserBackend::cdp`). Some CDP state belongs to that session, not to
//! the caller:
//!
//! * `Network.enable` / `Network.disable` (and every other domain): one caller's `disable`
//!   switches the domain off for everybody (`net.capture off` used to end the traffic log of
//!   `record.start {traffic: true}`);
//! * `Fetch.enable {patterns}` *replaces* the pattern list, `Fetch.disable` releases every
//!   paused request, and every event subscriber sees every `Fetch.requestPaused`, so two
//!   interceptors overwrite each other's patterns and race to answer the same request.
//!
//! [`CdpShare`] (one per browser, in the registry's `Extensions`) fixes both:
//!
//! * **domain refcount** ([`CdpShare::acquire`] / [`CdpShare::release`], module [`domains`]):
//!   each user holds a domain under its own name; `<Domain>.enable` is sent for the first
//!   holder (and again when the merged parameters grow), `<Domain>.disable` only when the last
//!   holder lets go;
//! * **Fetch coordinator** ([`CdpShare::set_fetch`], module [`fetch`]): owns the tab's only
//!   `Fetch.enable` (with the union of every client's patterns), reads every
//!   `Fetch.requestPaused` once, offers it to the clients in a fixed order (adblock gate → net
//!   rules/breakpoints → default continue) and makes sure each pause gets exactly one answer.
//!
//! Rule for the rest of the code base: **never send `<Domain>.enable/disable` or
//! `Fetch.enable/disable` directly, and never answer a `Fetch.requestPaused` you read from
//! `cdp_events` yourself** — go through this crate.

pub mod attach;
pub mod domains;
pub mod fetch;
mod glob;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use bmcp_core::{BackendResult, BrowserBackend, Ctx, TabId};
use serde_json::Value;

pub use fetch::{
    Answer, AnswerError, Answered, FetchClient, FetchHandler, FetchStats, ORDER_ADBLOCK, ORDER_NET,
    PausedRequest, Verdict,
};

/// Shared CDP state of every tab. One instance per browser (see the crate docs).
#[derive(Default)]
pub struct CdpShare {
    tabs: Mutex<HashMap<TabId, Arc<TabShare>>>,
}

/// One tab's shared CDP state.
pub(crate) struct TabShare {
    pub tab: TabId,
    pub backend: Arc<dyn BrowserBackend>,
    pub domains: domains::TabDomains,
    pub attach: attach::TabAttach,
    pub fetch: fetch::TabFetch,
}

impl TabShare {
    /// One CDP call on this tab.
    pub async fn cdp(&self, method: &str, params: Value) -> BackendResult<Value> {
        self.backend.cdp(self.tab, method.to_string(), params).await
    }
}

impl CdpShare {
    /// The shared instance from the call's context (created on first use).
    pub fn from_ctx(ctx: &Ctx) -> Arc<CdpShare> {
        ctx.extensions.get_or_insert_with(CdpShare::default)
    }

    /// The state of one tab, created on first use with `backend`.
    fn tab(&self, backend: &Arc<dyn BrowserBackend>, tab: TabId) -> Arc<TabShare> {
        lock(&self.tabs)
            .entry(tab)
            .or_insert_with(|| {
                Arc::new(TabShare {
                    tab,
                    backend: backend.clone(),
                    domains: domains::TabDomains::default(),
                    attach: attach::TabAttach::default(),
                    fetch: fetch::TabFetch::default(),
                })
            })
            .clone()
    }

    /// The state of one tab if anything was ever done there.
    fn existing(&self, tab: TabId) -> Option<Arc<TabShare>> {
        lock(&self.tabs).get(&tab).cloned()
    }

    // ---------------------------------------------------------------- domains

    /// Hold CDP domain `domain` (e.g. "Network") of `tab` for `holder` (e.g. "net",
    /// "record:rec-1"). Sends `<domain>.enable` with the merged parameters of all holders when
    /// this is the first holder or the merged parameters changed. Holding twice under the same
    /// name counts once (the newer parameters replace the older ones).
    pub async fn acquire(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        domain: &str,
        holder: &str,
        params: Value,
    ) -> BackendResult<()> {
        let ts = self.tab(backend, tab);
        domains::acquire(&ts, domain, holder, params).await
    }

    /// Let go of `domain` for `holder`. Sends `<domain>.disable` when nobody holds it any more
    /// (never for [`domains::NEVER_DISABLE`] domains). Releasing what is not held is a no-op.
    pub async fn release(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        domain: &str,
        holder: &str,
    ) -> BackendResult<()> {
        let ts = self.tab(backend, tab);
        domains::release(&ts, domain, holder).await
    }

    /// Who holds `domain` on `tab` now (sorted).
    pub fn holders(&self, tab: TabId, domain: &str) -> Vec<String> {
        self.existing(tab)
            .map(|ts| ts.domains.holders(domain))
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- auto-attach

    /// Hold `Target.setAutoAttach` on a session of `tab` (`None` = the page; `Some` = a
    /// flattened child session) for `holder` (see [`attach`]). `wait_for_debugger`: new
    /// targets should start paused — then this holder must resume every paused target it
    /// sees attach (`Runtime.runIfWaitingForDebugger`). The session gets pausing when any
    /// holder wants it. Holding again under the same name replaces the setting.
    ///
    /// Returns true when this call switched auto-attach on, so Chromium announced the existing
    /// targets (`Target.attachedToTarget`) to whoever subscribed before. False = it was on
    /// already: a holder that needs the existing targets calls
    /// [`Self::reannounce_auto_attach`].
    pub async fn hold_auto_attach(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        session: Option<String>,
        holder: &str,
        wait_for_debugger: bool,
    ) -> BackendResult<bool> {
        let ts = self.tab(backend, tab);
        attach::hold(&ts, session, holder, wait_for_debugger).await
    }

    /// Let go of auto-attach on a session; switched off when nobody holds it any more.
    pub async fn release_auto_attach(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        session: Option<String>,
        holder: &str,
    ) -> BackendResult<()> {
        let ts = self.tab(backend, tab);
        attach::unhold(&ts, session, holder).await
    }

    /// Turn auto-attach of a held session off and on again with the merged setting:
    /// Chromium detaches and re-announces every target (after lost attach events).
    pub async fn reannounce_auto_attach(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        session: Option<String>,
    ) -> BackendResult<()> {
        let ts = self.tab(backend, tab);
        attach::reannounce(&ts, session).await
    }

    /// A child session detached: forget its auto-attach state.
    pub fn forget_session(&self, tab: TabId, session: &str) {
        if let Some(ts) = self.existing(tab) {
            attach::forget_session(&ts, session);
        }
    }

    /// Who holds auto-attach on a session of `tab` (sorted).
    pub fn auto_attach_holders(&self, tab: TabId, session: Option<String>) -> Vec<String> {
        self.existing(tab)
            .map(|ts| ts.attach.holders(&session))
            .unwrap_or_default()
    }

    /// `waitForDebuggerOnStart` in effect on a session (`None` = auto-attach off).
    pub fn auto_attach_waits(&self, tab: TabId, session: Option<String>) -> Option<bool> {
        self.existing(tab)
            .and_then(|ts| ts.attach.waiting(&session))
    }

    // ---------------------------------------------------------------- Fetch

    /// Register (or replace) a Fetch client on `tab` and update `Fetch.enable` to the union of
    /// all clients' patterns. A client with no patterns is removed (like [`Self::clear_fetch`]).
    /// On error the previous registration is restored.
    pub async fn set_fetch(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        client: FetchClient,
    ) -> BackendResult<()> {
        let ts = self.tab(backend, tab);
        fetch::set_client(&ts, client).await
    }

    /// Remove the Fetch client `name` from `tab`; `Fetch.disable` when no client is left.
    pub async fn clear_fetch(
        &self,
        backend: &Arc<dyn BrowserBackend>,
        tab: TabId,
        name: &str,
    ) -> BackendResult<()> {
        let ts = self.tab(backend, tab);
        fetch::clear_client(&ts, name).await
    }

    /// Answer a paused request that a handler held ([`Verdict::Hold`]) — or any request still
    /// paused. Exactly one answer per pause is ever sent: a second call for the same pause
    /// returns [`AnswerError::NotPaused`]. If the answer fails it is replaced by a plain
    /// continue (the request must not hang) and the original error is returned.
    pub async fn answer(
        &self,
        tab: TabId,
        request_id: &str,
        answer: Answer,
        by: &str,
    ) -> Result<(), AnswerError> {
        match self.existing(tab) {
            Some(ts) => fetch::answer(&ts, request_id, answer, by).await,
            None => Err(AnswerError::NotPaused),
        }
    }

    /// The patterns `Fetch.enable` was last sent with on `tab` (empty = Fetch is off).
    pub fn fetch_patterns(&self, tab: TabId) -> Vec<Value> {
        self.existing(tab)
            .map(|ts| ts.fetch.enabled_patterns())
            .unwrap_or_default()
    }

    /// Counters of the tab's Fetch coordinator.
    pub fn fetch_stats(&self, tab: TabId) -> FetchStats {
        self.existing(tab)
            .map(|ts| ts.fetch.stats())
            .unwrap_or_default()
    }
}

/// Lock a std mutex; a poisoned lock (a panic elsewhere) still holds usable data here.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
