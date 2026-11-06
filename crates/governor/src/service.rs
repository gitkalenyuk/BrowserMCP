//! [`Governor`]: the long-lived state (per-tab mode, idle clocks, guards, decision log) and
//! the background service that runs the policy.
//!
//! The service ([`run`]) does two things:
//!
//! * listens to the event bus — `tab.activated` (wake a frozen/discarded tab, start the idle
//!   clock of the one that was shown), `tab.updated` (a discarded tab navigated by the user
//!   is live again), `tab.closed`, `command.started` (a command on a tab = "in use"; wakes it),
//!   `record.started/stopped` (recorded tabs are protected);
//! * every `tick_secs` runs [`crate::policy::decide`] and applies its plan.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use bmcp_core::events::topics as core_topics;
use bmcp_core::{
    BackendError, BrowserBackend, CmdError, DataDir, ErrorCode, Event, EventBus, Extensions, Retry,
    TabId, TabInfo,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::actions::{self, Saved};
use crate::config::{CONFIG_FILE, GovernorConfig, Limits, Profile};
use crate::memory::{MemorySource, ProcInfo, SystemMemory, to_mb};
use crate::policy::{self, Action, Kind, TabView};
use crate::topics;

/// How many decisions `governor.status` shows.
const KEEP_DECISIONS: usize = 100;

/// Something that can say "do not freeze or discard this tab". Other crates register one
/// with [`Governor::add_guard`] (e.g. an agent's tab lock, a job's working tabs). A plain
/// closure `|tab: &TabInfo| -> Option<String>` is a guard too.
pub trait TabGuard: Send + Sync + 'static {
    /// `Some(reason)` = leave this tab alone; `None` = this guard does not mind.
    fn protect(&self, tab: &TabInfo) -> Option<String>;
}

impl<F> TabGuard for F
where
    F: Fn(&TabInfo) -> Option<String> + Send + Sync + 'static,
{
    fn protect(&self, tab: &TabInfo) -> Option<String> {
        self(tab)
    }
}

/// `tabs.freeze` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FreezeMode {
    /// Pause the page (keeps memory, stops CPU).
    Freeze,
    /// Unload the page, keep URL/title/scroll to restore it (frees memory).
    Discard,
    /// Undo either: resume a frozen page, reload a discarded one.
    Thaw,
}

/// A tab's state as reported on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TabState {
    Live,
    Frozen,
    Discarded,
}

/// Our knowledge of one tab.
#[derive(Debug, Clone)]
enum Mode {
    Live,
    Frozen,
    Discarded(Saved),
}

impl Mode {
    fn state(&self) -> TabState {
        match self {
            Mode::Live => TabState::Live,
            Mode::Frozen => TabState::Frozen,
            Mode::Discarded(_) => TabState::Discarded,
        }
    }
}

#[derive(Debug, Clone)]
struct Track {
    mode: Mode,
    /// Since when the tab is in the background without commands.
    idle_since: Instant,
    /// When a command last targeted it.
    last_command: Option<Instant>,
    /// Bus seq of our `tab.discarded` event; older `tab.updated` events are stale.
    discard_seq: u64,
}

impl Track {
    fn new(now: Instant) -> Track {
        Track {
            mode: Mode::Live,
            idle_since: now,
            last_command: None,
            discard_seq: 0,
        }
    }
}

/// One entry of the decision log.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Decision {
    pub ts_ms: u64,
    pub tab: TabId,
    /// freeze | discard | thaw
    pub action: FreezeMode,
    pub reason: String,
    /// False when nothing had to be done (already in that state).
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of one freeze/discard/thaw.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Outcome {
    pub tab: TabId,
    /// The tab's state afterwards.
    pub state: TabState,
    /// False when the tab already was in that state.
    pub changed: bool,
    /// What a discarded tab remembers (URL, title, scroll).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved: Option<Saved>,
}

#[derive(Default)]
struct State {
    overrides: GovernorConfig,
    tabs: HashMap<TabId, Track>,
    /// The tab that was shown last (to start its idle clock when another one is activated).
    active: Option<TabId>,
    /// Running recordings: recording id → tab.
    recordings: HashMap<String, TabId>,
    log: VecDeque<Decision>,
}

/// The governor. One per browser, in the registry's `Extensions` ([`Governor::shared`]).
pub struct Governor {
    config_path: Option<PathBuf>,
    memory: Arc<dyn MemorySource>,
    guards: RwLock<Vec<Arc<dyn TabGuard>>>,
    state: Mutex<State>,
    /// Held while one tab is being frozen/discarded/restored: one operation at a time, so the
    /// policy and a manual `tabs.freeze` never race on the same tab.
    ops: tokio::sync::Mutex<()>,
}

impl Governor {
    /// A governor reading overrides from `config_path` (none = defaults, nothing saved).
    pub fn new(config_path: Option<PathBuf>, memory: Arc<dyn MemorySource>) -> Governor {
        let overrides = config_path
            .as_deref()
            .map(GovernorConfig::load)
            .unwrap_or_default();
        Governor {
            config_path,
            memory,
            guards: RwLock::new(Vec::new()),
            state: Mutex::new(State {
                overrides,
                ..State::default()
            }),
            ops: tokio::sync::Mutex::new(()),
        }
    }

    /// The shared instance, created on first use with the real machine's memory and
    /// `<data>/config/governor.json` (when a `DataDir` is in the extensions).
    pub fn shared(ext: &Extensions) -> Arc<Governor> {
        if let Some(gov) = ext.get::<Governor>() {
            return gov;
        }
        // Read the DataDir before `get_or_insert_with`: its closure runs under the map's lock.
        let path = ext
            .get::<DataDir>()
            .map(|data| data.config().join(CONFIG_FILE));
        ext.get_or_insert_with(|| Governor::new(path, Arc::new(SystemMemory)))
    }

    /// Register a guard; it is asked before every freeze/discard (manual and automatic).
    pub fn add_guard(&self, guard: Arc<dyn TabGuard>) {
        self.guards
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(guard);
    }

    /// The limits in force now (profile from RAM unless overridden).
    pub fn limits(&self) -> Limits {
        let auto = Profile::for_total_ram(self.memory.read().total);
        self.lock().overrides.limits(auto)
    }

    /// The stored overrides.
    pub fn overrides(&self) -> GovernorConfig {
        self.lock().overrides.clone()
    }

    /// Merge `patch` into the overrides (`reset` first clears them all) and save the file.
    pub fn configure(&self, patch: &GovernorConfig, reset: bool) -> std::io::Result<Limits> {
        let overrides = {
            let mut st = self.lock();
            if reset {
                st.overrides = GovernorConfig::default();
            }
            st.overrides.merge(patch);
            st.overrides.clone()
        };
        if let Some(path) = &self.config_path {
            overrides.save(path)?;
        }
        Ok(self.limits())
    }

    /// The tab's state as far as we know (`Live` for tabs we never touched).
    pub fn state_of(&self, tab: TabId) -> TabState {
        self.lock()
            .tabs
            .get(&tab)
            .map_or(TabState::Live, |t| t.mode.state())
    }

    /// The decision log, oldest first.
    pub fn decisions(&self) -> Vec<Decision> {
        self.lock().log.iter().cloned().collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn mode_of(&self, tab: TabId) -> Mode {
        self.lock()
            .tabs
            .get(&tab)
            .map_or(Mode::Live, |t| t.mode.clone())
    }

    fn set_mode(&self, tab: TabId, mode: Mode) {
        let mut st = self.lock();
        st.tabs
            .entry(tab)
            .or_insert_with(|| Track::new(Instant::now()))
            .mode = mode;
    }

    /// Why this tab must stay live, or `None`. `live` = the page runs (only then the audio
    /// check is asked over CDP; a frozen page plays nothing).
    pub async fn protection(
        &self,
        backend: &dyn BrowserBackend,
        tab: &TabInfo,
        live: bool,
    ) -> Option<String> {
        if tab.active {
            return Some("active tab".into());
        }
        if let Some(holder) = &tab.locked_by {
            return Some(format!("locked by {holder}"));
        }
        let busy = self.limits().busy();
        {
            let st = self.lock();
            if st.recordings.values().any(|t| *t == tab.id) {
                return Some("being recorded".into());
            }
            if let Some(last) = st.tabs.get(&tab.id).and_then(|t| t.last_command)
                && last.elapsed() < busy
            {
                return Some(format!(
                    "in use (last command {}s ago)",
                    last.elapsed().as_secs()
                ));
            }
        }
        let guards = self
            .guards
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(reason) = guards.iter().find_map(|g| g.protect(tab)) {
            return Some(reason);
        }
        if live && actions::plays_audio(backend, tab.id).await {
            return Some("playing audio".into());
        }
        None
    }

    /// Freeze, discard or thaw one tab (manual `tabs.freeze` and the policy both end here).
    /// Freeze/discard refuse protected tabs; thaw always works.
    pub async fn apply(
        &self,
        backend: &dyn BrowserBackend,
        events: &EventBus,
        tab: TabId,
        mode: FreezeMode,
        reason: &str,
    ) -> Result<Outcome, CmdError> {
        let _op = self.ops.lock().await;
        let result = self.apply_locked(backend, events, tab, mode, reason).await;
        let entry = Decision {
            ts_ms: now_ms(),
            tab,
            action: mode,
            reason: reason.to_string(),
            changed: result.as_ref().is_ok_and(|o| o.changed),
            error: result.as_ref().err().map(|e| e.message.clone()),
        };
        {
            let mut st = self.lock();
            st.log.push_back(entry);
            while st.log.len() > KEEP_DECISIONS {
                st.log.pop_front();
            }
        }
        if let Err(err) = &result {
            tracing::debug!(%tab, ?mode, %err, "governor action failed");
        }
        result
    }

    async fn apply_locked(
        &self,
        backend: &dyn BrowserBackend,
        events: &EventBus,
        tab: TabId,
        mode: FreezeMode,
        reason: &str,
    ) -> Result<Outcome, CmdError> {
        let info = backend
            .list_tabs()
            .await?
            .into_iter()
            .find(|t| t.id == tab)
            .ok_or(BackendError::TabNotFound(tab))?;
        let current = self.mode_of(tab);
        let unchanged = |mode: &Mode| Outcome {
            tab,
            state: mode.state(),
            changed: false,
            saved: match mode {
                Mode::Discarded(saved) => Some(saved.clone()),
                _ => None,
            },
        };
        let changed = |state: TabState, saved: Option<Saved>| Outcome {
            tab,
            state,
            changed: true,
            saved,
        };

        match (mode, current) {
            // ---- freeze
            (FreezeMode::Freeze, current @ (Mode::Frozen | Mode::Discarded(_))) => {
                Ok(unchanged(&current))
            }
            (FreezeMode::Freeze, Mode::Live) => {
                self.refuse_if_protected(backend, &info, true).await?;
                actions::set_frozen(backend, tab, true).await?;
                self.set_mode(tab, Mode::Frozen);
                events.emit(topics::TAB_FROZEN, json!({ "id": tab, "reason": reason }));
                Ok(changed(TabState::Frozen, None))
            }
            // ---- discard
            (FreezeMode::Discard, current @ Mode::Discarded(_)) => Ok(unchanged(&current)),
            (FreezeMode::Discard, current) => {
                if !actions::is_discardable(&info.url) {
                    return Err(CmdError::invalid_params(format!(
                        "tab {tab} shows {}; only web pages (http, https, file) are discarded",
                        info.url
                    ))
                    .with_hint("Use mode \"freeze\" for this tab."));
                }
                let frozen = matches!(current, Mode::Frozen);
                self.refuse_if_protected(backend, &info, !frozen).await?;
                let saved = match actions::discard(backend, &info, frozen).await {
                    Ok(saved) => saved,
                    Err(err) => {
                        // `discard` resumes a frozen page first; we are not sure where it
                        // stopped, so call the tab live (a thaw is harmless).
                        self.set_mode(tab, Mode::Live);
                        return Err(err.into());
                    }
                };
                // Emit while holding the state lock, so the event handler (which takes the
                // same lock) never sees the new mode without its `discard_seq`.
                let mut st = self.lock();
                let track = st
                    .tabs
                    .entry(tab)
                    .or_insert_with(|| Track::new(Instant::now()));
                track.mode = Mode::Discarded(saved.clone());
                track.discard_seq = events.emit(
                    topics::TAB_DISCARDED,
                    json!({ "id": tab, "url": saved.url, "title": saved.title, "reason": reason }),
                );
                Ok(changed(TabState::Discarded, Some(saved)))
            }
            // ---- thaw
            (FreezeMode::Thaw, Mode::Live) => Ok(unchanged(&Mode::Live)),
            (FreezeMode::Thaw, Mode::Frozen) => {
                actions::set_frozen(backend, tab, false).await?;
                self.set_mode(tab, Mode::Live);
                events.emit(topics::TAB_THAWED, json!({ "id": tab, "reason": reason }));
                Ok(changed(TabState::Live, None))
            }
            (FreezeMode::Thaw, Mode::Discarded(saved)) => {
                // Live first: the `tab.updated` events of this navigation are ours.
                self.set_mode(tab, Mode::Live);
                actions::restore(backend, tab, &saved).await?;
                events.emit(
                    topics::TAB_RESTORED,
                    json!({ "id": tab, "url": saved.url, "reason": reason }),
                );
                Ok(changed(TabState::Live, Some(saved)))
            }
        }
    }

    async fn refuse_if_protected(
        &self,
        backend: &dyn BrowserBackend,
        info: &TabInfo,
        live: bool,
    ) -> Result<(), CmdError> {
        match self.protection(backend, info, live).await {
            None => Ok(()),
            Some(why) => Err(CmdError::new(
                ErrorCode::NotInteractable,
                format!("tab {} is protected: {why}", info.id),
            )
            .with_retry(Retry::No)
            .with_hint("Active, locked, recorded, recently used and audible tabs stay live.")),
        }
    }

    /// React to one bus event. Slow work (thaw, restore) is spawned, so the event loop
    /// never waits for a page load.
    pub fn on_event(
        self: &Arc<Self>,
        backend: &Arc<dyn BrowserBackend>,
        events: &EventBus,
        ev: &Event,
    ) {
        let tab_of = |v: &Value| v.as_i64().map(|n| TabId(n as i32));
        match ev.topic.as_str() {
            core_topics::TAB_ACTIVATED => {
                let Some(id) = tab_of(&ev.data["id"]) else {
                    return;
                };
                let now = Instant::now();
                let wake = {
                    let mut st = self.lock();
                    // The tab that was shown until now starts its idle clock.
                    if let Some(prev) = st.active.replace(id)
                        && prev != id
                        && let Some(t) = st.tabs.get_mut(&prev)
                    {
                        t.idle_since = now;
                    }
                    let t = st.tabs.entry(id).or_insert_with(|| Track::new(now));
                    t.idle_since = now;
                    !matches!(t.mode, Mode::Live)
                };
                if wake {
                    self.spawn_apply(backend, events, id, "activated".into());
                }
            }
            core_topics::TAB_CLOSED => {
                if let Some(id) = tab_of(&ev.data["id"]) {
                    self.lock().tabs.remove(&id);
                }
            }
            core_topics::TAB_UPDATED => self.on_tab_updated(backend, events, ev),
            core_topics::COMMAND_STARTED => {
                let name = ev.data["name"].as_str().unwrap_or_default();
                // `tab` is only filled for tab-mutating commands; read-only ones (page.text,
                // page.snapshot) have it in their arguments.
                let Some(id) = tab_of(&ev.data["tab"]).or_else(|| tab_of(&ev.data["args"]["tab"]))
                else {
                    return;
                };
                // Our own commands are not "using" a tab.
                if name == "tabs.freeze" || name.starts_with("governor.") {
                    return;
                }
                // Commands that navigate or close the tab anyway must not race with a
                // restore navigation (nav.goto could end on the old URL). For a discarded tab
                // `tab.updated` marks it live as soon as the new page shows.
                let navigates = name.starts_with("nav.") || name == "tabs.close";
                let wake = {
                    let mut st = self.lock();
                    let now = Instant::now();
                    let t = st.tabs.entry(id).or_insert_with(|| Track::new(now));
                    t.last_command = Some(now);
                    t.idle_since = now;
                    match t.mode {
                        Mode::Live => false,
                        Mode::Frozen => true,
                        Mode::Discarded(_) => !navigates,
                    }
                };
                if wake {
                    self.spawn_apply(backend, events, id, format!("command {name}"));
                }
            }
            "record.started" => {
                if let (Some(rec), Some(id)) = (ev.data["id"].as_str(), tab_of(&ev.data["tab"])) {
                    self.lock().recordings.insert(rec.to_string(), id);
                }
            }
            "record.stopped" => {
                if let Some(rec) = ev.data["id"].as_str() {
                    self.lock().recordings.remove(rec);
                }
            }
            _ => {}
        }
    }

    /// A discarded tab that shows something else than the placeholder was navigated by the
    /// user (the "click to restore" link, the omnibox, Back): it is live again.
    fn on_tab_updated(
        self: &Arc<Self>,
        backend: &Arc<dyn BrowserBackend>,
        events: &EventBus,
        ev: &Event,
    ) {
        let (Some(id), Some(url)) = (
            ev.data["id"].as_i64().map(|n| TabId(n as i32)),
            ev.data["url"].as_str(),
        ) else {
            return;
        };
        if actions::is_placeholder(url) {
            return;
        }
        let saved = {
            let mut st = self.lock();
            let Some(track) = st.tabs.get_mut(&id) else {
                return;
            };
            let Mode::Discarded(saved) = &track.mode else {
                return;
            };
            // Events from before the discard finished still carry the old URL.
            if ev.seq <= track.discard_seq {
                return;
            }
            let saved = saved.clone();
            track.mode = Mode::Live;
            saved
        };
        events.emit(
            topics::TAB_RESTORED,
            json!({ "id": id, "url": url, "reason": "navigated" }),
        );
        // Back on the same page: scroll to where the user was, once it has loaded.
        if url == saved.url {
            let backend = backend.clone();
            tokio::spawn(async move {
                for _ in 0..40 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let loaded =
                        backend.list_tabs().await.ok().and_then(|tabs| {
                            tabs.into_iter().find(|t| t.id == id).map(|t| !t.loading)
                        });
                    match loaded {
                        Some(true) => {
                            actions::restore_scroll(&*backend, id, &saved).await;
                            break;
                        }
                        Some(false) => continue,
                        None => break, // closed
                    }
                }
            });
        }
    }

    fn spawn_apply(
        self: &Arc<Self>,
        backend: &Arc<dyn BrowserBackend>,
        events: &EventBus,
        tab: TabId,
        reason: String,
    ) {
        let (gov, backend, events) = (self.clone(), backend.clone(), events.clone());
        tokio::spawn(async move {
            let _ = gov
                .apply(&*backend, &events, tab, FreezeMode::Thaw, &reason)
                .await;
        });
    }

    /// One policy round: refresh the idle clocks from the tab list, decide, apply.
    pub async fn tick(&self, backend: &dyn BrowserBackend, events: &EventBus) {
        let limits = self.limits();
        if !limits.enabled {
            return;
        }
        let Ok(tabs) = backend.list_tabs().await else {
            return;
        };
        let now = Instant::now();
        let tracks: HashMap<TabId, Track> = {
            let mut st = self.lock();
            st.tabs.retain(|id, _| tabs.iter().any(|t| t.id == *id));
            for t in &tabs {
                let track = st.tabs.entry(t.id).or_insert_with(|| Track::new(now));
                if t.active {
                    track.idle_since = now;
                }
            }
            if let Some(active) = tabs.iter().find(|t| t.active) {
                st.active = Some(active.id);
            }
            st.tabs.clone()
        };
        let mut views = Vec::with_capacity(tabs.len());
        for t in &tabs {
            let track = &tracks[&t.id];
            let kind = match track.mode {
                Mode::Live => Kind::Live,
                Mode::Frozen => Kind::Frozen,
                Mode::Discarded(_) => Kind::Discarded,
            };
            let protected = kind != Kind::Discarded
                && self
                    .protection(backend, t, kind == Kind::Live)
                    .await
                    .is_some();
            views.push(TabView {
                id: t.id,
                kind,
                idle: now.saturating_duration_since(track.idle_since),
                protected,
                discardable: actions::is_discardable(&t.url),
            });
        }
        for planned in policy::decide(&views, &limits, self.memory.read()) {
            let mode = match planned.action {
                Action::Freeze => FreezeMode::Freeze,
                Action::Discard => FreezeMode::Discard,
            };
            let _ = self
                .apply(backend, events, planned.tab, mode, &planned.reason)
                .await;
        }
    }

    /// Everything `governor.status` shows.
    pub async fn status(&self, backend: &dyn BrowserBackend) -> Result<Status, CmdError> {
        let limits = self.limits();
        let mem = self.memory.read();
        // Listing all processes takes a while; keep it off the async worker threads.
        let memory = self.memory.clone();
        let processes = tokio::task::spawn_blocking(move || memory.processes())
            .await
            .unwrap_or_default();
        let tabs = backend.list_tabs().await?;
        let tracks = self.lock().tabs.clone();
        let mut out = Vec::with_capacity(tabs.len());
        for t in &tabs {
            let track = tracks.get(&t.id);
            let mode = track.map_or(Mode::Live, |t| t.mode.clone());
            let live = matches!(mode, Mode::Live);
            let js_heap_mb = if live {
                actions::js_heap(backend, t.id).await.map(to_mb)
            } else {
                None
            };
            out.push(TabStatus {
                id: t.id,
                title: t.title.clone(),
                url: t.url.clone(),
                state: mode.state(),
                idle_secs: if t.active {
                    0
                } else {
                    track.map_or(0, |t| t.idle_since.elapsed().as_secs())
                },
                protected: match mode {
                    Mode::Discarded(_) => None,
                    _ => self.protection(backend, t, live).await,
                },
                js_heap_mb,
                saved: match mode {
                    Mode::Discarded(saved) => Some(saved),
                    _ => None,
                },
            });
        }
        let browser_mb = (processes.iter().map(|p| p.mb).sum::<f64>() * 10.0).round() / 10.0;
        Ok(Status {
            memory: MemoryStatus {
                total_mb: mem.total >> 20,
                available_mb: mem.available >> 20,
                free_pct: (mem.free_pct() * 10.0).round() / 10.0,
                pressure: mem.under_pressure(limits.pressure_free_pct),
                browser_mb,
                renderers: processes.iter().filter(|p| p.kind == "renderer").count(),
                processes,
            },
            limits,
            overrides: self.overrides(),
            tabs: out,
            decisions: self.decisions(),
        })
    }
}

/// `governor.status` output.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Status {
    pub limits: Limits,
    /// What `governor.config` changed (the rest are profile defaults).
    pub overrides: GovernorConfig,
    pub memory: MemoryStatus,
    pub tabs: Vec<TabStatus>,
    /// Recent freeze/discard/thaw decisions, oldest first (manual and automatic).
    pub decisions: Vec<Decision>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MemoryStatus {
    pub total_mb: u64,
    pub available_mb: u64,
    pub free_pct: f64,
    /// Available RAM is below `limits.pressure_free_pct`.
    pub pressure: bool,
    /// Working set of the browser and all its helper processes.
    pub browser_mb: f64,
    /// Number of renderer processes.
    pub renderers: usize,
    pub processes: Vec<ProcInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TabStatus {
    pub id: TabId,
    pub title: String,
    pub url: String,
    pub state: TabState,
    /// Seconds in the background without commands (0 for the active tab).
    pub idle_secs: u64,
    /// Why the policy leaves it alone (null = it may be frozen/discarded).
    pub protected: Option<String>,
    /// JS heap of the page (`Runtime.getHeapUsage`); null when frozen/discarded. Renderer
    /// processes cannot be mapped to tabs from here, so this is the per-tab estimate.
    pub js_heap_mb: Option<f64>,
    /// What a discarded tab will restore.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved: Option<Saved>,
}

/// The background service: event reactions + a policy tick every `tick_secs`. Runs until the
/// event bus closes (in practice: until the tokio runtime shuts down).
pub async fn run(gov: Arc<Governor>, backend: Arc<dyn BrowserBackend>, events: EventBus) {
    let mut rx = events.subscribe();
    let mut next_tick = Instant::now() + gov.limits().tick();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next_tick) => {
                gov.tick(&*backend, &events).await;
                next_tick = Instant::now() + gov.limits().tick();
            }
            ev = rx.recv() => match ev {
                Ok(ev) => gov.on_event(&backend, &events, &ev),
                Err(RecvError::Lagged(n)) => tracing::debug!(n, "governor missed events"),
                Err(RecvError::Closed) => break,
            },
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
