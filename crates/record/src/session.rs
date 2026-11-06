//! One running recording: a tokio task that listens to the event bus (API calls) and to the
//! tab's CDP events (recorder binding, navigations, traffic), and writes the steps.
//!
//! **Dedupe.** `page.act` sends real trusted input, so the recorder script reports the same
//! click/typing again as a "human" event. Each API call that acts on the page opens a time
//! window `[started − 50 ms, finished + 400 ms]`; a human event whose page time falls in an
//! open or recent window is logged with `dup_of: <API step>` (and gives the API step its CSS
//! path, input type and secret flag). Human events are held back [`HOLD`] before they are
//! judged, so a `command.started` that is still in the bus channel cannot be missed.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_cdpshare::CdpShare;
use bmcp_core::events::topics;
use bmcp_core::{BrowserBackend, CdpEvent, Ctx, Event, TabId};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, oneshot};
use tokio::time::Instant;

use crate::api;
use crate::human::{self, HumanEvent, Mode};
use crate::model::{RecordingMeta, Source, Step};
use crate::store::{Log, lock, now_ms, write_meta};
use crate::video::VideoRecorder;

/// How long a human event waits before it is matched against API windows.
const HOLD: Duration = Duration::from_millis(300);

/// A recording's holder name in the tab's domain refcount (`bmcp-cdpshare`).
pub fn domain_holder(recording_id: &str) -> String {
    format!("record:{recording_id}")
}

/// Give back the `Page` and `Network` domains a recording held (best effort: the tab may be
/// gone). `Network` is only disabled when nobody else (e.g. `net.capture`) still holds it.
pub async fn release_domains(
    backend: &Arc<dyn BrowserBackend>,
    tab: TabId,
    holder: &str,
    share: &CdpShare,
) {
    for domain in ["Network", "Page"] {
        if let Err(err) = share.release(backend, tab, domain, holder).await {
            tracing::debug!(%err, domain, "recorder: releasing the domain failed");
        }
    }
}
/// Wait this long for the recorder's `hello` through the binding before falling back to polling.
const HELLO_TIMEOUT: Duration = Duration::from_millis(1500);
/// Poll interval in queue mode (and the housekeeping tick in binding mode).
const TICK: Duration = Duration::from_millis(250);
/// A human event this much before an API call started still counts as caused by it.
const WINDOW_BEFORE_MS: u64 = 50;
/// … and this much after it finished (events can be delivered late).
const WINDOW_AFTER_MS: u64 = 400;
/// API windows we remember.
const KEEP_WINDOWS: usize = 32;

/// The time an API call ran, and the steps it produced.
#[derive(Debug)]
struct Window {
    call_id: u64,
    seqs: Vec<u64>,
    started_ms: u64,
    finished_ms: Option<u64>,
    /// API steps already matched by a human event (each is matched once).
    matched: Vec<u64>,
}

impl Window {
    fn covers(&self, t: u64) -> bool {
        let start = self.started_ms.saturating_sub(WINDOW_BEFORE_MS);
        let end = self.finished_ms.map_or(u64::MAX, |f| f + WINDOW_AFTER_MS);
        (start..=end).contains(&t)
    }

    /// How far `t` is outside the call's real run time (0 = while it ran).
    fn distance(&self, t: u64) -> u64 {
        let end = self.finished_ms.unwrap_or(u64::MAX);
        if t < self.started_ms {
            self.started_ms - t
        } else {
            t.saturating_sub(end)
        }
    }
}

/// Does the human action `human` look like the API action `api`?
fn same_kind(api: &str, human: &str) -> bool {
    api == human
        || matches!(
            (api, human),
            ("type", "fill") | ("check" | "uncheck", "click") | ("dblclick", "click")
        )
}

/// Everything the session task needs.
pub struct Session {
    pub ctx: Ctx,
    pub tab: TabId,
    pub dir: PathBuf,
    pub log: Arc<Mutex<Log>>,
    pub meta: Arc<Mutex<RecordingMeta>>,
    pub video: Option<VideoRecorder>,
    pub traffic: Option<std::io::BufWriter<std::fs::File>>,
    /// Main frame id (navigations of iframes are ignored).
    pub main_frame: String,
    /// Current URL of the tab.
    pub url: String,
    /// `addScriptToEvaluateOnNewDocument` identifier, removed at the end.
    pub script_id: String,
    pub mode: Mode,
    /// A `bindingCalled` arrived: the binding works without `Runtime.enable`.
    pub binding_works: bool,
    pub started: Instant,
    /// Isolated world context for polling (reset on navigation).
    pub context: Option<i64>,
    /// A new document loaded: its recorder world needs the binding (binding mode).
    rebind: bool,
    windows: VecDeque<Window>,
    /// call_id → seqs of the API steps waiting for `command.finished`.
    calls: HashMap<u64, Vec<u64>>,
    /// Human events waiting [`HOLD`] (received at, step).
    held: VecDeque<(Instant, Step)>,
    /// Running ref → locator resolutions (see [`Session::finalize_api_step`]).
    resolving: tokio::task::JoinSet<()>,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: Ctx,
        tab: TabId,
        dir: PathBuf,
        log: Arc<Mutex<Log>>,
        meta: Arc<Mutex<RecordingMeta>>,
        main_frame: String,
        url: String,
        script_id: String,
    ) -> Session {
        Session {
            ctx,
            tab,
            dir,
            log,
            meta,
            video: None,
            traffic: None,
            main_frame,
            url,
            script_id,
            mode: Mode::Binding,
            binding_works: false,
            started: Instant::now(),
            context: None,
            rebind: false,
            windows: VecDeque::new(),
            calls: HashMap::new(),
            held: VecDeque::new(),
            resolving: tokio::task::JoinSet::new(),
        }
    }

    fn backend(&self) -> Arc<dyn BrowserBackend> {
        self.ctx.backend.clone()
    }

    /// The task body: run until `stop` fires (or the tab closes), then finish the files.
    pub async fn run(
        mut self,
        mut bus: broadcast::Receiver<Event>,
        mut cdp: broadcast::Receiver<CdpEvent>,
        mut stop: oneshot::Receiver<()>,
    ) -> RecordingMeta {
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                _ = &mut stop => break,
                event = bus.recv() => match event {
                    Ok(event) => self.on_bus(event),
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "recorder fell behind the event bus");
                    }
                    Err(RecvError::Closed) => break,
                },
                event = cdp.recv() => match event {
                    Ok(event) => self.on_cdp(event),
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "recorder fell behind the CDP events");
                    }
                    // The tab was closed.
                    Err(RecvError::Closed) => break,
                },
                _ = tick.tick() => self.on_tick().await,
            }
        }
        self.finish(&mut bus, &mut cdp).await
    }

    // ------------------------------------------------------------ API calls

    fn on_bus(&mut self, event: Event) {
        let data = &event.data;
        let name = data["name"].as_str().unwrap_or_default();
        if !api::is_logged(name) {
            return;
        }
        // An explicit other tab is not ours; no tab means "the default tab", taken as ours.
        if let Some(tab) = data["tab"].as_i64()
            && tab != i64::from(self.tab.0)
        {
            return;
        }
        let call_id = data["call_id"].as_u64().unwrap_or(0);
        match event.topic.as_str() {
            topics::COMMAND_STARTED => {
                let steps = api::steps_for_call(data, event.ts_ms, &self.url);
                let seqs: Vec<u64> = {
                    let mut log = lock(&self.log);
                    steps.into_iter().map(|s| log.push(s)).collect()
                };
                if api::acts_on_page(name) {
                    self.windows.push_back(Window {
                        call_id,
                        seqs: seqs.clone(),
                        started_ms: event.ts_ms,
                        finished_ms: None,
                        matched: Vec::new(),
                    });
                    while self.windows.len() > KEEP_WINDOWS {
                        self.windows.pop_front();
                    }
                }
                self.calls.insert(call_id, seqs);
            }
            topics::COMMAND_FINISHED => {
                let Some(seqs) = self.calls.remove(&call_id) else {
                    return;
                };
                if let Some(w) = self.windows.iter_mut().find(|w| w.call_id == call_id) {
                    w.finished_ms = Some(event.ts_ms);
                }
                for seq in seqs {
                    self.finalize_api_step(seq, Some(data));
                }
                self.save();
            }
            _ => {}
        }
    }

    /// Fill in the result, then resolve the ref to role + name, redact secrets and un-pend.
    ///
    /// Resolving waits for the tab's perception state, which the *next* `page.act` may hold
    /// for a second, so it runs in its own task: the session keeps reading events meanwhile.
    fn finalize_api_step(&mut self, seq: u64, finished: Option<&Value>) {
        let reference = {
            let mut log = lock(&self.log);
            let Some(step) = log.get_mut(seq) else {
                return;
            };
            if let (Some(call), Some(data)) = (&mut step.command, finished) {
                call.ok = data["ok"].as_bool();
                call.error = data["error"].as_str().map(str::to_string);
                call.duration_ms = data["duration_ms"].as_u64();
            }
            step.target.as_ref().and_then(|t| t.ref_.clone())
        };
        let (ctx, tab, log) = (self.ctx.clone(), self.tab, self.log.clone());
        let id = self.id();
        self.resolving.spawn(async move {
            let resolved = match &reference {
                Some(r) => api::resolve_target(&ctx, tab, r).await,
                None => None,
            };
            if let Some(step) = apply_resolution(&log, seq, resolved) {
                emit_step(&ctx.events, &id, &step);
            }
        });
    }

    /// The recording id (for events).
    fn id(&self) -> String {
        lock(&self.meta).id.clone()
    }
}

/// `record.step {id, step}` on the event bus (the UI's Recording panel shows it live).
fn emit_step(events: &bmcp_core::EventBus, id: &str, step: &Step) {
    events.emit("record.step", json!({ "id": id, "step": step }));
}

/// Second half of [`Session::finalize_api_step`]: store the locator, redact, un-pend, save.
/// Returns the finished step.
fn apply_resolution(
    log: &Mutex<Log>,
    seq: u64,
    resolved: Option<(crate::model::Target, bool)>,
) -> Option<Step> {
    {
        let mut log = lock(log);
        let step = log.get_mut(seq)?;
        let mut pwd = false;
        if let Some((mut target, is_pwd)) = resolved {
            pwd = is_pwd;
            // Perception's role/name win; keep what the recorder script added (css, …).
            if let Some(old) = &step.target {
                target.merge_missing(old);
            }
            step.target = Some(target);
        }
        api::redact_if_secret(step, pwd);
        step.pending = false;
        let step = step.clone();
        if let Err(err) = log.save() {
            tracing::warn!(%err, "recorder: cannot write actions.jsonl");
        }
        Some(step)
    }
}

impl Session {
    // ------------------------------------------------------------ CDP events

    fn on_cdp(&mut self, event: CdpEvent) {
        if event.session_id.is_some() {
            return; // iframes / workers
        }
        let p = &event.params;
        match event.method.as_str() {
            "Runtime.bindingCalled" if p["name"] == human::BINDING => {
                self.binding_works = true;
                self.on_payload(p["payload"].as_str().unwrap_or_default());
            }
            "Page.frameNavigated" if p["frame"]["parentId"].is_null() => {
                let url = p["frame"]["url"].as_str().unwrap_or_default().to_string();
                if let Some(id) = p["frame"]["id"].as_str() {
                    self.main_frame = id.to_string();
                }
                self.context = None;
                self.rebind = true;
                self.url.clone_from(&url);
                let step = Step {
                    ts_ms: now_ms(),
                    source: Source::Human,
                    action: "navigated".into(),
                    url: Some(url),
                    ..Step::default()
                };
                self.held.push_back((Instant::now(), step));
            }
            "Page.navigatedWithinDocument" if p["frameId"] == self.main_frame.as_str() => {
                self.url = p["url"].as_str().unwrap_or_default().to_string();
            }
            "Network.requestWillBeSent" | "Network.responseReceived" => self.on_traffic(&event),
            _ => {}
        }
    }

    fn on_payload(&mut self, payload: &str) {
        match human::parse_event(payload) {
            Some(HumanEvent::Hello) | None => {}
            Some(HumanEvent::Step(step)) => self.held.push_back((Instant::now(), *step)),
        }
    }

    fn on_traffic(&mut self, event: &CdpEvent) {
        let Some(out) = &mut self.traffic else {
            return;
        };
        let p = &event.params;
        let line = if event.method == "Network.requestWillBeSent" {
            json!({ "ts_ms": now_ms(), "event": "request", "id": p["requestId"],
                    "method": p["request"]["method"], "url": p["request"]["url"], "type": p["type"] })
        } else {
            json!({ "ts_ms": now_ms(), "event": "response", "id": p["requestId"],
                    "status": p["response"]["status"], "mime": p["response"]["mimeType"],
                    "url": p["response"]["url"] })
        };
        let _ = writeln!(out, "{line}");
    }

    // ------------------------------------------------------------ human events

    /// Judge held human events that are old enough.
    fn commit_held(&mut self, all: bool) {
        let mut changed = false;
        while let Some((received, _)) = self.held.front() {
            if !all && received.elapsed() < HOLD {
                break;
            }
            let (_, step) = self.held.pop_front().expect("front exists");
            self.commit_human(step);
            changed = true;
        }
        if changed {
            self.save();
        }
    }

    fn commit_human(&mut self, mut step: Step) {
        step.url = step.url.or_else(|| Some(self.url.clone()));
        let mut log = lock(&self.log);
        // Of the windows that cover the event, the one it is closest to (back-to-back calls
        // overlap in their grace periods).
        let t = step.ts_ms;
        let window = self
            .windows
            .iter_mut()
            .filter(|w| w.covers(t))
            .min_by_key(|w| w.distance(t));
        if let Some(window) = window {
            // The API step this event belongs to: the first unmatched one of the same kind.
            let api_seq = window
                .seqs
                .iter()
                .copied()
                .find(|seq| {
                    !window.matched.contains(seq)
                        && log
                            .get_mut(*seq)
                            .is_some_and(|a| same_kind(&a.action, &step.action))
                })
                .unwrap_or(window.seqs[0]);
            if step.is_element_action() {
                window.matched.push(api_seq);
            }
            step.dup_of = Some(api_seq);
            if let Some(api_step) = log.get_mut(api_seq)
                && api_step.is_element_action()
            {
                if let (Some(dom), Some(target)) = (&step.target, &mut api_step.target) {
                    target.merge_missing(dom);
                }
                if step.secret {
                    api_step.secret = true;
                    api::redact_if_secret(api_step, true);
                }
            }
        }
        let seq = log.push(step);
        drop(log);
        if let Some(step) = lock(&self.log).get_mut(seq).cloned() {
            emit_step(&self.ctx.events, &self.id(), &step);
        }
    }

    // ------------------------------------------------------------ housekeeping

    async fn on_tick(&mut self) {
        self.commit_held(false);
        if self.mode == Mode::Binding && self.rebind {
            self.bind_new_document().await;
        }
        if self.mode == Mode::Binding {
            if self.binding_works {
                self.set_capture("binding");
            } else if self.started.elapsed() >= HELLO_TIMEOUT {
                self.fall_back_to_polling().await;
            }
        }
        if self.mode == Mode::Queue {
            self.poll().await;
        }
    }

    fn set_capture(&self, how: &str) {
        let mut meta = lock(&self.meta);
        if meta.human_capture != how {
            meta.human_capture = how.to_string();
            let _ = write_meta(&self.dir, &meta);
        }
    }

    /// After a navigation: give the new document's recorder world the binding, then collect
    /// what the script queued before it had one. Retried on the next tick while the new
    /// document is not ready.
    async fn bind_new_document(&mut self) {
        let backend = self.backend();
        let Ok(context) = human::bind(&backend, self.tab).await else {
            return;
        };
        self.rebind = false;
        if let Ok(items) = human::drain(&backend, self.tab, context).await {
            for item in items {
                self.on_payload(&item);
            }
        }
    }

    /// No `bindingCalled` came: switch the page script to its queue and poll it.
    async fn fall_back_to_polling(&mut self) {
        tracing::info!(tab = %self.tab, "recorder: no Runtime.bindingCalled, polling instead");
        let backend = self.backend();
        human::uninstall(&backend, self.tab, &self.script_id).await;
        match human::install(&backend, self.tab, Mode::Queue).await {
            Ok(id) => self.script_id = id,
            Err(err) => tracing::warn!(%err, "recorder: re-install in queue mode failed"),
        }
        if let Ok(context) = human::world_context(&backend, self.tab).await {
            let _ = human::evaluate_in(
                &backend,
                self.tab,
                context,
                "globalThis.__bmcpRecorder && (globalThis.__bmcpRecorder.mode = 'queue')",
            )
            .await;
            self.context = Some(context);
        }
        self.mode = Mode::Queue;
        self.set_capture("poll");
    }

    /// Queue mode: read the page's queue.
    async fn poll(&mut self) {
        let backend = self.backend();
        let context = match self.context {
            Some(c) => c,
            None => match human::world_context(&backend, self.tab).await {
                Ok(c) => {
                    self.context = Some(c);
                    c
                }
                Err(_) => return,
            },
        };
        match human::drain(&backend, self.tab, context).await {
            Ok(items) => {
                for item in items {
                    self.on_payload(&item);
                }
            }
            // The document went away; find the new context next time.
            Err(_) => self.context = None,
        }
    }

    fn save(&self) {
        let log = lock(&self.log);
        if let Err(err) = log.save() {
            tracing::warn!(%err, "recorder: cannot write actions.jsonl");
        }
        let count = log.written().count();
        drop(log);
        lock(&self.meta).steps = count;
    }

    /// Final flush, remove the recorder, stop the video, write everything.
    async fn finish(
        mut self,
        bus: &mut broadcast::Receiver<Event>,
        cdp: &mut broadcast::Receiver<CdpEvent>,
    ) -> RecordingMeta {
        // Events that arrived before the stop but were not read yet still belong in the log.
        while let Ok(event) = bus.try_recv() {
            self.on_bus(event);
        }
        while let Ok(event) = cdp.try_recv() {
            self.on_cdp(event);
        }
        let backend = self.backend();
        // Half-typed fields: report them now.
        if let Ok(context) = human::world_context(&backend, self.tab).await {
            let _ = human::evaluate_in(
                &backend,
                self.tab,
                context,
                "globalThis.__bmcpRecorder && globalThis.__bmcpRecorder.flush()",
            )
            .await;
            if self.mode == Mode::Queue {
                if let Ok(items) = human::drain(&backend, self.tab, context).await {
                    for item in items {
                        self.on_payload(&item);
                    }
                }
            } else {
                // Give the binding events of the flush a moment to arrive.
                let until = Instant::now() + Duration::from_millis(200);
                while let Ok(Ok(event)) = tokio::time::timeout_at(until, cdp.recv()).await {
                    self.on_cdp(event);
                }
            }
            human::stop_in_page(&backend, self.tab, context).await;
        }
        human::uninstall(&backend, self.tab, &self.script_id).await;

        // API calls still running: keep them, without a result.
        let open: Vec<u64> = self.calls.drain().flat_map(|(_, seqs)| seqs).collect();
        for seq in open {
            self.finalize_api_step(seq, None);
        }
        while self.resolving.join_next().await.is_some() {}
        self.commit_held(true);
        if !self.binding_works && self.mode == Mode::Binding {
            // Stopped before the hello timeout: we never learned whether bindings work.
            self.set_capture("binding (unconfirmed)");
        }

        let video = match self.video.take() {
            Some(video) => {
                let report = video.stop().await;
                if let Some(err) = &report.error {
                    tracing::warn!(%err, "recording video");
                }
                Some(report)
            }
            None => None,
        };
        if let Some(mut out) = self.traffic.take() {
            let _ = out.flush();
        }
        let holder = domain_holder(&lock(&self.meta).id);
        let share = CdpShare::from_ctx(&self.ctx);
        release_domains(&backend, self.tab, &holder, &share).await;
        self.save();
        let mut meta = lock(&self.meta);
        meta.stopped_ms = Some(now_ms());
        if let Some(report) = video
            && report.bytes == 0
        {
            meta.video = None;
        }
        let _ = write_meta(&self.dir, &meta);
        meta.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_cover_a_grace_period() {
        let mut w = Window {
            call_id: 1,
            seqs: vec![1],
            started_ms: 1000,
            finished_ms: None,
            matched: Vec::new(),
        };
        assert!(w.covers(960) && w.covers(1_000_000));
        assert!(!w.covers(900));
        w.finished_ms = Some(2000);
        assert!(w.covers(2400) && !w.covers(2401));
        assert_eq!(w.distance(1500), 0);
        assert_eq!(w.distance(960), 40);
        assert_eq!(w.distance(2100), 100);
    }

    #[test]
    fn kinds_match_like_the_page_sees_them() {
        assert!(same_kind("fill", "fill"));
        assert!(same_kind("type", "fill"));
        assert!(same_kind("check", "click"));
        assert!(!same_kind("fill", "click"));
    }
}
