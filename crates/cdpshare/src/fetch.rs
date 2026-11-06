//! The Fetch coordinator: the one owner of a tab's `Fetch` domain and its paused requests.
//!
//! **Clients.** A subsystem registers a [`FetchClient`]: a name, an order, its
//! `Fetch.RequestPattern`s and a [`FetchHandler`]. The coordinator sends `Fetch.enable` with
//! the union of all clients' patterns, and `Fetch.disable` only when the last client is gone.
//!
//! **One reader, one answer.** The coordinator has its own subscription to the tab's CDP
//! events and is the only code that reads `Fetch.requestPaused`. For each pause it asks the
//! clients whose patterns match, in order ([`ORDER_ADBLOCK`] → [`ORDER_NET`] → …):
//! * [`Verdict::Pass`] — not mine / no objection: ask the next client;
//! * [`Verdict::Answer`] — the coordinator sends that answer, nobody else is asked;
//! * [`Verdict::Hold`] — the client keeps the request (a breakpoint) and answers later with
//!   [`crate::CdpShare::answer`].
//!
//! If every client passes, the request is continued unchanged. Every pause is tracked until it
//! is answered, and the entry is removed *before* the answer is sent, so a second answer for
//! the same pause can never go out (it gets [`AnswerError::NotPaused`]). A failed answer is
//! replaced by a plain continue. A handler that panics or takes longer than
//! [`HANDLER_TIMEOUT`] gets its request continued; a held request that nobody answers within
//! [`HOLD_LIMIT`] is continued too. After the answer, every client that was interested hears
//! about it ([`FetchHandler::done`]) — for statistics and to drop stale state.
//!
//! **Lost events (`RecvError::Lagged`).** A lost `Fetch.requestPaused` would leave its request
//! paused forever (the page hangs). `Fetch.disable` + `enable` would release it, but it would
//! also release every request a client holds on purpose (breakpoints), so we do not do that.
//! Instead:
//! 1. lag is made unlikely: the coordinator's event loop does O(1) work per event (it only
//!    filters `Fetch.requestPaused` and spawns a task), and no client reads pauses from its
//!    own (slower) subscription any more — a `net.capture` listener that falls behind on a
//!    heavy page no longer loses pauses;
//! 2. if the coordinator itself lags, it **sweeps**: Chromium names Fetch requests
//!    `interception-job-<N>.<hop>`, with N counting up. We remember the highest N seen; after a
//!    short grace period ([`SWEEP_GRACE`], so in-flight events can still arrive) we send
//!    `Fetch.continueRequest` for every `interception-job-<N>.0` from a little below the last
//!    id seen before the lag up to the highest one seen since (or a bounded guess when none
//!    came), skipping ids that are pending (being decided or held) and ids already answered
//!    for good. For ids that are not paused in this session Chromium just answers "Invalid
//!    InterceptionId", which we ignore. This is best effort: it relies on Chromium's id scheme
//!    and covers the first hop of each request (redirect hops `.1`, `.2` are not guessed), and
//!    a swept request goes through without being offered to the clients (e.g. not blocked).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bmcp_core::{BackendError, BackendResult, BoxFuture, CdpEvents, TabId};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::glob::pattern_matches;
use crate::{TabShare, lock};

/// Order of the adblock gate: blocking decisions come first.
pub const ORDER_ADBLOCK: u8 = 10;
/// Order of net's rules and breakpoints: after adblock, so a blocked request never pauses.
pub const ORDER_NET: u8 = 20;

/// A handler that has not decided after this long gets its request continued.
pub const HANDLER_TIMEOUT: Duration = Duration::from_secs(30);
/// A held request that nobody answered after this long is continued (net's own breakpoint
/// timeout is 60 s; this is only the safety net behind it).
pub const HOLD_LIMIT: Duration = Duration::from_secs(120);
/// Wait this long after a lag before sweeping, so events still in flight can arrive.
pub const SWEEP_GRACE: Duration = Duration::from_millis(500);
/// The sweep starts this many job numbers below the last one seen before the lag (events of
/// one session are not strictly in id order).
const SWEEP_LOOKBACK: u64 = 32;
/// Without newer ids, the sweep guesses at most this many job numbers past the last one seen.
const SWEEP_AHEAD: u64 = 256;
/// At most this many ids per sweep.
const SWEEP_MAX: usize = 1024;
/// How many "answered for good" ids are remembered for the sweep.
const FINISHED_KEEP: usize = 4096;
/// Chromium's prefix of Fetch request ids.
const JOB_PREFIX: &str = "interception-job-";

/// `by` of answers the coordinator sends itself when every client passed.
pub const BY_DEFAULT: &str = "default";
/// `by` when a handler panicked or timed out.
pub const BY_FALLBACK: &str = "fallback";
/// `by` when a held request reached [`HOLD_LIMIT`].
pub const BY_HOLD_LIMIT: &str = "hold-limit";
/// `by` when the last client left and `Fetch.disable` released everything.
pub const BY_DISABLE: &str = "Fetch.disable";
/// `by` when the tab closed (Chromium dropped its requests).
pub const BY_TAB_CLOSED: &str = "tab-closed";

/// One paused request, as the handlers see it.
#[derive(Debug, Clone)]
pub struct PausedRequest {
    pub tab: TabId,
    /// `Fetch` request id (not the `Network` one; that is `params.networkId`).
    pub request_id: String,
    /// The `Fetch.requestPaused` parameters.
    pub params: Value,
    /// Paused after the response headers arrived (`responseStatusCode`/`responseErrorReason`).
    pub response_stage: bool,
    /// When the coordinator read the event.
    pub received: Instant,
}

impl PausedRequest {
    fn new(tab: TabId, params: Value) -> Option<PausedRequest> {
        let request_id = params["requestId"].as_str()?.to_string();
        let response_stage = params.get("responseStatusCode").is_some()
            || params.get("responseErrorReason").is_some();
        Some(PausedRequest {
            tab,
            request_id,
            params,
            response_stage,
            received: Instant::now(),
        })
    }

    pub fn url(&self) -> &str {
        self.params["request"]["url"].as_str().unwrap_or_default()
    }

    pub fn method(&self) -> &str {
        self.params["request"]["method"]
            .as_str()
            .unwrap_or_default()
    }

    /// CDP `Network.ResourceType` such as "Script" or "XHR".
    pub fn resource_type(&self) -> &str {
        self.params["resourceType"].as_str().unwrap_or_default()
    }
}

/// The answer to one paused request: a `Fetch.*` method and its parameters. `requestId` is
/// filled in by the coordinator.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// `Fetch.continueRequest`, `continueResponse`, `fulfillRequest` or `failRequest`.
    pub method: &'static str,
    pub params: Value,
}

impl Answer {
    pub fn new(method: &'static str, params: Value) -> Answer {
        Answer { method, params }
    }

    /// Let it through unchanged.
    pub fn continue_request() -> Answer {
        Answer::new("Fetch.continueRequest", json!({}))
    }

    /// Fail it with a CDP `Network.ErrorReason` such as "BlockedByClient".
    pub fn fail(reason: &str) -> Answer {
        Answer::new("Fetch.failRequest", json!({ "errorReason": reason }))
    }

    fn is_plain_continue(&self) -> bool {
        self.method == "Fetch.continueRequest"
            && self
                .params
                .as_object()
                .is_none_or(|p| p.keys().all(|k| k == "requestId"))
    }

    /// After this answer the request cannot pause again (no response stage follows).
    fn ends_request(&self) -> bool {
        matches!(self.method, "Fetch.failRequest" | "Fetch.fulfillRequest")
    }
}

/// What a handler decided about one paused request.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Not mine, or no objection: ask the next client (the last one continues it).
    Pass,
    /// Send this answer now.
    Answer(Answer),
    /// I keep it and answer later with [`crate::CdpShare::answer`].
    Hold,
}

/// How a paused request was answered in the end (see [`FetchHandler::done`]).
#[derive(Debug, Clone, Copy)]
pub struct Answered<'a> {
    /// Client name that answered, or one of the `BY_*` constants.
    pub by: &'a str,
    /// The CDP method of the answer ("" when Chromium released it: disable, tab closed).
    pub method: &'static str,
    /// Why the answer failed (then a plain continue was tried instead).
    pub error: Option<&'a str>,
    /// The request is no longer paused (answer or fallback continue went through).
    pub released: bool,
    /// A handler had held the request ([`Verdict::Hold`]) before this answer.
    pub held: bool,
}

/// A subsystem's part in a tab's request interception.
pub trait FetchHandler: Send + Sync + 'static {
    /// Decide about one paused request. Runs in its own task. Must not wait for anything the
    /// page renderer handles (`Runtime.*`, `Page.*Script*`, …): while a navigation request is
    /// paused, Chromium holds those back until the navigation commits.
    fn on_paused<'a>(&'a self, req: &'a PausedRequest) -> BoxFuture<'a, Verdict>;

    /// Called once the request was answered (by anybody), on every client whose patterns
    /// matched it. Keep it short: it runs in the answering task.
    fn done(&self, req: &PausedRequest, answered: &Answered<'_>) {
        let _ = (req, answered);
    }
}

/// One subsystem's registration on a tab.
#[derive(Clone)]
pub struct FetchClient {
    /// Unique per tab, e.g. "adblock" or "net"; also the `by` of its answers.
    pub name: String,
    /// Clients are asked in ascending order ([`ORDER_ADBLOCK`], [`ORDER_NET`]).
    pub order: u8,
    /// `Fetch.RequestPattern`s, e.g. `{"urlPattern": "*", "requestStage": "Request"}`.
    pub patterns: Vec<Value>,
    pub handler: Arc<dyn FetchHandler>,
}

/// Why [`crate::CdpShare::answer`] did not answer.
#[derive(Debug, Clone, PartialEq)]
pub enum AnswerError {
    /// The request is not paused (any more): already answered, released by the hold limit or
    /// `Fetch.disable`, or its tab closed.
    NotPaused,
    /// The answer failed; a plain continue was tried instead.
    Cdp(BackendError),
}

impl fmt::Display for AnswerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnswerError::NotPaused => write!(f, "the request is not paused any more"),
            AnswerError::Cdp(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AnswerError {}

/// Counters of one tab's coordinator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FetchStats {
    /// `Fetch.requestPaused` events read.
    pub paused: u64,
    /// Answers sent (including fallbacks).
    pub answered: u64,
    /// Events lost because the coordinator fell behind.
    pub lagged: u64,
    /// Requests released by a lag sweep.
    pub swept: u64,
    /// Requests paused right now (being decided or held).
    pub pending: usize,
}

/// One tab's Fetch state.
#[derive(Default)]
pub(crate) struct TabFetch {
    /// Held while changing clients and `Fetch.enable`, so those CDP calls never interleave.
    config: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Sorted by (order, name).
    clients: Vec<FetchClient>,
    /// Patterns `Fetch.enable` was last sent with; empty = Fetch is disabled.
    enabled: Vec<Value>,
    /// The event loop (runs while Fetch is enabled).
    task: Option<JoinHandle<()>>,
    /// Paused requests not answered yet, by request id.
    pending: HashMap<String, Pending>,
    next_serial: u64,
    /// Ids answered for good (they cannot pause again), newest last; for the lag sweep.
    finished: VecDeque<String>,
    finished_set: HashSet<String>,
    /// Highest Chromium job number seen (see the module docs on lag).
    max_job: Option<u64>,
    stats: FetchStats,
}

struct Pending {
    /// Tells a pause apart from a later pause with the same id (response stage).
    serial: u64,
    req: PausedRequest,
    /// Handlers whose patterns matched (they hear about the answer).
    interested: Vec<Arc<dyn FetchHandler>>,
    held: bool,
}

impl TabFetch {
    pub fn enabled_patterns(&self) -> Vec<Value> {
        lock(&self.state).enabled.clone()
    }

    pub fn stats(&self) -> FetchStats {
        let st = lock(&self.state);
        FetchStats {
            pending: st.pending.len(),
            ..st.stats
        }
    }
}

impl State {
    fn mark_finished(&mut self, id: &str) {
        if self.finished_set.insert(id.to_string()) {
            self.finished.push_back(id.to_string());
            if self.finished.len() > FINISHED_KEEP
                && let Some(old) = self.finished.pop_front()
            {
                self.finished_set.remove(&old);
            }
        }
    }

    fn sort_clients(&mut self) {
        self.clients
            .sort_by(|a, b| (a.order, &a.name).cmp(&(b.order, &b.name)));
    }
}

// -------------------------------------------------------------------- clients

pub(crate) async fn set_client(ts: &Arc<TabShare>, client: FetchClient) -> BackendResult<()> {
    let name = client.name.clone();
    let new = (!client.patterns.is_empty()).then_some(client);
    change_client(ts, &name, new).await
}

pub(crate) async fn clear_client(ts: &Arc<TabShare>, name: &str) -> BackendResult<()> {
    change_client(ts, name, None).await
}

/// Replace (or remove, `new = None`) client `name`, then bring `Fetch.enable` in line.
async fn change_client(
    ts: &Arc<TabShare>,
    name: &str,
    new: Option<FetchClient>,
) -> BackendResult<()> {
    let _config = ts.fetch.config.lock().await;
    let previous = {
        let mut st = lock(&ts.fetch.state);
        let previous = st
            .clients
            .iter()
            .position(|c| c.name == name)
            .map(|i| st.clients.remove(i));
        if let Some(new) = new {
            st.clients.push(new);
            st.sort_clients();
        }
        previous
    };
    if let Err(err) = sync(ts).await {
        // Keep the registration in line with what the browser really does.
        let mut st = lock(&ts.fetch.state);
        st.clients.retain(|c| c.name != name);
        if let Some(previous) = previous {
            st.clients.push(previous);
            st.sort_clients();
        }
        return Err(err);
    }
    Ok(())
}

/// All clients' patterns, deduplicated, in client order.
fn union(clients: &[FetchClient]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for pattern in clients.iter().flat_map(|c| &c.patterns) {
        if !out.contains(pattern) {
            out.push(pattern.clone());
        }
    }
    out
}

/// Make `Fetch.enable` match the clients. Call with the `config` lock held.
async fn sync(ts: &Arc<TabShare>) -> BackendResult<()> {
    let (wanted, current) = {
        let st = lock(&ts.fetch.state);
        (union(&st.clients), st.enabled.clone())
    };
    if wanted == current {
        return Ok(());
    }
    if wanted.is_empty() {
        ts.cdp("Fetch.disable", json!({})).await?;
        // Chromium released everything that was still paused.
        let (task, released) = {
            let mut st = lock(&ts.fetch.state);
            st.enabled.clear();
            let released: Vec<Pending> = st.pending.drain().map(|(_, p)| p).collect();
            (st.task.take(), released)
        };
        if let Some(task) = task {
            task.abort();
        }
        for p in released {
            notify_released(&p, BY_DISABLE, "Fetch.disable");
        }
        return Ok(());
    }
    ensure_task(ts).await?;
    if let Err(err) = ts.cdp("Fetch.enable", json!({ "patterns": wanted })).await {
        let mut st = lock(&ts.fetch.state);
        if st.enabled.is_empty()
            && let Some(task) = st.task.take()
        {
            task.abort();
        }
        return Err(err);
    }
    lock(&ts.fetch.state).enabled = wanted;
    Ok(())
}

/// Start the event loop if it is not running. Subscribes *before* Fetch is enabled, so no
/// pause can be missed.
async fn ensure_task(ts: &Arc<TabShare>) -> BackendResult<()> {
    if lock(&ts.fetch.state)
        .task
        .as_ref()
        .is_some_and(|t| !t.is_finished())
    {
        return Ok(());
    }
    let events = ts.backend.cdp_events(ts.tab).await?;
    let task = tokio::spawn(event_loop(ts.clone(), events));
    if let Some(old) = lock(&ts.fetch.state).task.replace(task) {
        old.abort();
    }
    Ok(())
}

// -------------------------------------------------------------------- pauses

/// Read the tab's events until it closes (or Fetch is disabled). O(1) work per event.
async fn event_loop(ts: Arc<TabShare>, mut events: CdpEvents) {
    loop {
        match events.recv().await {
            // Only the page target's own pauses: Fetch is enabled there, and a pause of a
            // child session (iframe, worker) could not be answered here anyway.
            Ok(event) if event.method == "Fetch.requestPaused" && event.session_id.is_none() => {
                if let Some(req) = PausedRequest::new(ts.tab, event.params)
                    && let Some((serial, interested)) = track(&ts, &req)
                {
                    tokio::spawn(dispatch(ts.clone(), req, serial, interested));
                }
            }
            Ok(_) => {}
            Err(RecvError::Lagged(missed)) => on_lag(&ts, missed),
            Err(RecvError::Closed) => {
                tab_closed(&ts);
                break;
            }
        }
    }
}

type Interested = Vec<(String, Arc<dyn FetchHandler>)>;

/// Start tracking a pause; returns its serial and the clients to ask, in order.
fn track(ts: &TabShare, req: &PausedRequest) -> Option<(u64, Interested)> {
    let mut st = lock(&ts.fetch.state);
    st.stats.paused += 1;
    if let Some(job) = job_number(&req.request_id) {
        st.max_job = Some(st.max_job.map_or(job, |m| m.max(job)));
    }
    if st.pending.contains_key(&req.request_id) {
        tracing::warn!(tab = %ts.tab, id = %req.request_id, "Fetch: duplicate requestPaused ignored");
        return None;
    }
    let interested: Interested = st
        .clients
        .iter()
        .filter(|c| {
            c.patterns
                .iter()
                .any(|p| pattern_matches(p, req.url(), req.resource_type(), req.response_stage))
        })
        .map(|c| (c.name.clone(), c.handler.clone()))
        .collect();
    st.next_serial += 1;
    let serial = st.next_serial;
    st.pending.insert(
        req.request_id.clone(),
        Pending {
            serial,
            req: req.clone(),
            interested: interested.iter().map(|(_, h)| h.clone()).collect(),
            held: false,
        },
    );
    Some((serial, interested))
}

/// Ask the clients in order until one answers or holds; continue when all pass.
async fn dispatch(ts: Arc<TabShare>, req: PausedRequest, serial: u64, interested: Interested) {
    let id = req.request_id.clone();
    for (name, handler) in interested {
        if !is_pending(&ts, &id, serial) {
            return; // answered meanwhile (e.g. Fetch was disabled)
        }
        // In a task of its own, so a panicking handler cannot strand the request.
        let mut call = {
            let req = req.clone();
            tokio::spawn(async move { handler.on_paused(&req).await })
        };
        let verdict = match tokio::time::timeout(HANDLER_TIMEOUT, &mut call).await {
            Ok(Ok(verdict)) => verdict,
            Ok(Err(err)) => {
                tracing::warn!(tab = %ts.tab, %id, client = %name, %err, "Fetch handler failed; continuing the request");
                let _ = answer_pause(
                    &ts,
                    &id,
                    Some(serial),
                    Answer::continue_request(),
                    BY_FALLBACK,
                )
                .await;
                return;
            }
            Err(_) => {
                call.abort();
                tracing::warn!(tab = %ts.tab, %id, client = %name, "Fetch handler timed out; continuing the request");
                let _ = answer_pause(
                    &ts,
                    &id,
                    Some(serial),
                    Answer::continue_request(),
                    BY_FALLBACK,
                )
                .await;
                return;
            }
        };
        match verdict {
            Verdict::Pass => {}
            Verdict::Answer(answer) => {
                let _ = answer_pause(&ts, &id, Some(serial), answer, &name).await;
                return;
            }
            Verdict::Hold => {
                hold(&ts, &id, serial);
                return;
            }
        }
    }
    let _ = answer_pause(
        &ts,
        &id,
        Some(serial),
        Answer::continue_request(),
        BY_DEFAULT,
    )
    .await;
}

fn is_pending(ts: &TabShare, id: &str, serial: u64) -> bool {
    lock(&ts.fetch.state)
        .pending
        .get(id)
        .is_some_and(|p| p.serial == serial)
}

/// Mark a pause as held and start its [`HOLD_LIMIT`] safety timer.
fn hold(ts: &Arc<TabShare>, id: &str, serial: u64) {
    match lock(&ts.fetch.state).pending.get_mut(id) {
        Some(p) if p.serial == serial => p.held = true,
        _ => return, // already answered
    }
    let ts = ts.clone();
    let id = id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(HOLD_LIMIT).await;
        let answered = answer_pause(
            &ts,
            &id,
            Some(serial),
            Answer::continue_request(),
            BY_HOLD_LIMIT,
        )
        .await;
        if answered.is_ok() {
            tracing::warn!(tab = %ts.tab, %id, "Fetch: held request was never answered; continued it");
        }
    });
}

/// Answer a pending pause (any pause with this id when `serial` is `None`).
pub(crate) async fn answer(
    ts: &TabShare,
    id: &str,
    answer: Answer,
    by: &str,
) -> Result<(), AnswerError> {
    answer_pause(ts, id, None, answer, by).await
}

async fn answer_pause(
    ts: &TabShare,
    id: &str,
    serial: Option<u64>,
    answer: Answer,
    by: &str,
) -> Result<(), AnswerError> {
    // Take it out first: whoever removes the entry is the only one who answers.
    let pending = {
        let mut st = lock(&ts.fetch.state);
        match st.pending.get(id) {
            Some(p) if serial.is_none_or(|s| s == p.serial) => {}
            _ => return Err(AnswerError::NotPaused),
        }
        st.pending.remove(id).ok_or(AnswerError::NotPaused)?
    };
    let mut params = match answer.params.clone() {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    params["requestId"] = json!(id);
    let sent = ts.cdp(answer.method, params).await;
    let (error, released) = match &sent {
        Ok(_) => (None, true),
        Err(err) => {
            // The request must not hang: fall back to a plain continue.
            let released = !answer.is_plain_continue()
                && ts
                    .cdp("Fetch.continueRequest", json!({ "requestId": id }))
                    .await
                    .is_ok();
            tracing::debug!(tab = %ts.tab, %id, method = answer.method, %by, %err, released, "Fetch answer failed");
            (Some(err.to_string()), released)
        }
    };
    {
        let mut st = lock(&ts.fetch.state);
        st.stats.answered += 1;
        let response_patterns = st.enabled.iter().any(|p| p["requestStage"] == "Response");
        if pending.req.response_stage || answer.ends_request() || !response_patterns {
            st.mark_finished(id);
        }
    }
    let info = Answered {
        by,
        method: answer.method,
        error: error.as_deref(),
        released,
        held: pending.held,
    };
    for handler in &pending.interested {
        handler.done(&pending.req, &info);
    }
    sent.map(|_| ()).map_err(AnswerError::Cdp)
}

/// Tell the interested clients that Chromium released a pause by itself.
fn notify_released(p: &Pending, by: &str, method: &'static str) {
    let info = Answered {
        by,
        method,
        error: None,
        released: true,
        held: p.held,
    };
    for handler in &p.interested {
        handler.done(&p.req, &info);
    }
}

/// The tab is gone: Chromium dropped its requests and the session.
fn tab_closed(ts: &TabShare) {
    let released: Vec<Pending> = {
        let mut st = lock(&ts.fetch.state);
        st.enabled.clear();
        st.task = None; // this very task; it ends right after
        st.pending.drain().map(|(_, p)| p).collect()
    };
    for p in released {
        notify_released(&p, BY_TAB_CLOSED, "");
    }
}

// -------------------------------------------------------------------- lost events

/// `interception-job-12.0` → 12.
fn job_number(id: &str) -> Option<u64> {
    id.strip_prefix(JOB_PREFIX)?.split('.').next()?.parse().ok()
}

fn on_lag(ts: &Arc<TabShare>, missed: u64) {
    let before = {
        let mut st = lock(&ts.fetch.state);
        st.stats.lagged += missed;
        st.max_job
    };
    tracing::warn!(tab = %ts.tab, missed, "Fetch: CDP events lost; sweeping for stranded requests");
    tokio::spawn(sweep(ts.clone(), before, missed));
}

/// Continue requests whose pause event was lost (see the module docs).
async fn sweep(ts: Arc<TabShare>, before: Option<u64>, missed: u64) {
    tokio::time::sleep(SWEEP_GRACE).await;
    let candidates: Vec<String> = {
        let st = lock(&ts.fetch.state);
        if st.enabled.is_empty() {
            return; // Fetch is off: Chromium released everything
        }
        let Some(base) = before.or(st.max_job) else {
            tracing::warn!(tab = %ts.tab, "Fetch: no Chromium request ids seen; cannot sweep");
            return;
        };
        let low = base.saturating_sub(SWEEP_LOOKBACK).max(1);
        let high = st
            .max_job
            .unwrap_or(base)
            .max(base + missed.min(SWEEP_AHEAD));
        (low..=high)
            .map(|job| format!("{JOB_PREFIX}{job}.0"))
            .filter(|id| !st.pending.contains_key(id) && !st.finished_set.contains(id))
            .take(SWEEP_MAX)
            .collect()
    };
    let mut released = 0;
    for id in candidates {
        // A late event may have arrived since: never touch a pause that is being handled.
        if lock(&ts.fetch.state).pending.contains_key(&id) {
            continue;
        }
        if ts
            .cdp("Fetch.continueRequest", json!({ "requestId": id }))
            .await
            .is_ok()
        {
            released += 1;
            lock(&ts.fetch.state).mark_finished(&id);
        }
    }
    lock(&ts.fetch.state).stats.swept += released;
    if released > 0 {
        tracing::warn!(tab = %ts.tab, released, "Fetch: continued requests whose pause event was lost");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_numbers_follow_chromium_ids() {
        assert_eq!(job_number("interception-job-12.0"), Some(12));
        assert_eq!(job_number("interception-job-7.3"), Some(7));
        assert_eq!(job_number("f1"), None);
        assert_eq!(job_number("interception-job-x.0"), None);
    }

    #[test]
    fn plain_continue_is_recognized() {
        assert!(Answer::continue_request().is_plain_continue());
        assert!(
            Answer::new("Fetch.continueRequest", json!({ "requestId": "1" })).is_plain_continue()
        );
        assert!(
            !Answer::new("Fetch.continueRequest", json!({ "url": "https://b.test/" }))
                .is_plain_continue()
        );
        assert!(!Answer::fail("Aborted").is_plain_continue());
        assert!(Answer::fail("Aborted").ends_request());
    }
}
