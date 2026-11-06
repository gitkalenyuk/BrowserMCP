//! BrowserMCP jobs: saved scripts, mass runs, exports, cron triggers and the self-healing hook.
//!
//! * [`scripts`] — the script library in `data/scripts/*.ts` (`script.list/get/save/delete`);
//! * [`host`] — runs one script in a `bmcp-script-host` process (`script.run`) and reads its
//!   output: `browser.output({...})` prints `##bmcp:result {json}`, errors are parsed into
//!   `{code, command, message, line}`;
//! * [`data`] — input rows from CSV / JSON / Excel / inline;
//! * [`runner`] — mass runs: a queue with N worker slots, one script process per row, retries
//!   with backoff, pause/resume/cancel, the heal pause (`run.start`, `run.*`);
//! * [`store`] — runs, rows and triggers in `data/db/runs.sqlite` (restart-safe);
//! * [`export`] — results to CSV / Excel / JSON / SQLite / webhook (`run.export`);
//! * [`triggers`] — cron triggers that fire `script.run` (`trigger.*`);
//! * [`debug`] — the step debugger: a `bmcp-script-host --inspect pipe` process per session
//!   (`script.debug_start`, `script.debug_cmd`, events `script.debug_*`);
//! * [`commands`] — the registry commands (toolsets `script` and `run`).
//!
//! The app creates one [`JobsService`], puts it into the registry's `Extensions` and calls
//! [`register`]:
//!
//! ```no_run
//! # async fn demo(data: bmcp_core::DataDir, events: bmcp_core::EventBus) {
//! let mut registry = bmcp_core::Registry::new();
//! let jobs = bmcp_jobs::JobsService::start(bmcp_jobs::JobsConfig::new(&data, 47800), events)
//!     .expect("runs database");
//! registry.extensions().insert(jobs);
//! bmcp_jobs::register(&mut registry);
//! # }
//! ```

pub mod commands;
pub mod data;
pub mod debug;
pub mod export;
pub mod host;
pub mod runner;
pub mod scripts;
pub mod store;
pub mod triggers;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bmcp_core::{CmdError, DataDir, EventBus, Registry};
use tokio_util::sync::CancellationToken;

use crate::runner::RunCtl;
pub use crate::runner::{HealHook, HealTrigger};
use crate::scripts::ScriptLib;
pub use crate::store::HealMode;
use crate::store::Store;

/// Event topics emitted on the [`EventBus`]. WS clients get them with
/// `events.subscribe {"topics":["run.*","trigger.*"]}`.
pub mod topics {
    /// `{id, script, total, parallel}`
    pub const RUN_STARTED: &str = "run.started";
    /// `{id, done, ok, failed, needs_heal, cancelled, running, pending, total}` after every row.
    pub const RUN_PROGRESS: &str = "run.progress";
    /// `{id, row, error: {code, command?, message, line?}, attempts}` when a row failed for good.
    pub const RUN_ROW_FAILED: &str = "run.row_failed";
    /// `{id, row, script, error, step, line, mode}`: a row broke on a page.* selector error
    /// and the run paused; answer with `run.heal_apply`. `mode` is `ask` or `auto`.
    pub const RUN_HEAL_NEEDED: &str = "run.heal_needed";
    /// `{id, status, total, ok, failed, cancelled, needs_heal, duration_ms}`
    pub const RUN_FINISHED: &str = "run.finished";
    /// `{id, script, ok, exit_code, duration_ms, error?}` after a cron trigger ran its script.
    pub const TRIGGER_FIRED: &str = "trigger.fired";
}

/// Paths and knobs of the jobs service.
#[derive(Debug, Clone)]
pub struct JobsConfig {
    /// `data/scripts`
    pub scripts_dir: PathBuf,
    /// `data/runs` (exports go to `data/runs/<id>/`)
    pub runs_dir: PathBuf,
    /// `data/db/runs.sqlite`
    pub db_path: PathBuf,
    /// Explicit `bmcp-script-host` binary; `None` = search next to the browser executable
    /// (see [`host::locate_host`]).
    pub host_exe: Option<PathBuf>,
    /// Where scripts send `browser.*` calls.
    pub ws_url: String,
    /// First retry waits this long, then 2×, 4×, … (capped at 30 s).
    pub retry_backoff: Duration,
    /// Limit parallelism by CPU count and free RAM (on by default; tests turn it off).
    pub auto_limit: bool,
    /// Hard upper limit of worker slots per run.
    pub max_parallel: usize,
    /// Extra time after a script's timeout before its host process is killed (the host
    /// normally stops on its own and exits with 124). Default [`host::DEFAULT_KILL_GRACE`].
    pub kill_grace: Duration,
}

impl JobsConfig {
    /// The standard layout inside the portable data folder, talking to the browser's API
    /// on `api_port`.
    pub fn new(data: &DataDir, api_port: u16) -> Self {
        JobsConfig {
            scripts_dir: data.root().join("scripts"),
            runs_dir: data.root().join("runs"),
            db_path: data.db().join("runs.sqlite"),
            host_exe: None,
            ws_url: format!("ws://127.0.0.1:{api_port}/v1/ws"),
            retry_backoff: Duration::from_secs(1),
            auto_limit: true,
            max_parallel: 32,
            kill_grace: host::DEFAULT_KILL_GRACE,
        }
    }
}

/// The long-lived state behind all jobs commands. Cheap to clone (one `Arc`).
#[derive(Clone)]
pub struct JobsService {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub config: JobsConfig,
    /// Starts as `config.ws_url`; the app updates it once the API server is listening.
    pub ws_url: RwLock<String>,
    pub scripts: ScriptLib,
    pub store: Store,
    pub events: EventBus,
    /// Runs that have a live driver task (running or paused in this process).
    pub runs: Mutex<HashMap<String, Arc<RunCtl>>>,
    /// Triggers whose script is running right now (a trigger never overlaps itself).
    pub firing: Mutex<HashSet<String>>,
    /// Cancelled on browser shutdown: kills script processes but keeps rows `running` in the
    /// database, so the next start resumes them as `pending`.
    pub shutdown: CancellationToken,
    /// Live `script.debug_start` sessions.
    pub debug: debug::DebugSessions,
    /// Takes a page snapshot when a heal request opens (installed by `crates/heal`).
    pub heal_hook: RwLock<Option<HealHook>>,
}

impl JobsService {
    /// Open the database and folders. Runs that were running when the browser last stopped
    /// become `paused` (resume them with `run.resume`). Does not start the trigger scheduler.
    pub fn open(config: JobsConfig, events: EventBus) -> Result<Self, CmdError> {
        for dir in [&config.scripts_dir, &config.runs_dir] {
            std::fs::create_dir_all(dir)
                .map_err(|e| CmdError::internal(format!("{}: {e}", dir.display())))?;
        }
        let store = Store::open(&config.db_path)?;
        let interrupted = store.recover_interrupted()?;
        if interrupted > 0 {
            tracing::info!(interrupted, "runs paused after restart");
        }
        Ok(JobsService {
            inner: Arc::new(Inner {
                ws_url: RwLock::new(config.ws_url.clone()),
                scripts: ScriptLib::new(&config.scripts_dir),
                store,
                events,
                runs: Mutex::new(HashMap::new()),
                firing: Mutex::new(HashSet::new()),
                shutdown: CancellationToken::new(),
                debug: Default::default(),
                heal_hook: RwLock::new(None),
                config,
            }),
        })
    }

    /// [`open`](Self::open) and start the cron scheduler. Must be called inside a tokio
    /// runtime.
    pub fn start(config: JobsConfig, events: EventBus) -> Result<Self, CmdError> {
        let service = Self::open(config, events)?;
        tokio::spawn(triggers::scheduler(service.inner.clone()));
        Ok(service)
    }

    /// Point scripts at the actual API endpoint (e.g. after the server picked its port).
    pub fn set_ws_url(&self, url: impl Into<String>) {
        *self.inner.ws_url.write().unwrap_or_else(|e| e.into_inner()) = url.into();
    }

    /// Install the hook that snapshots the page of a row that needs healing (see
    /// [`HealHook`]). Its text is stored in the runs database before `run.heal_needed` goes
    /// out; read it back with [`heal_snapshot`](Self::heal_snapshot).
    pub fn set_heal_hook(&self, hook: HealHook) {
        *self
            .inner
            .heal_hook
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    /// The page snapshot stored for a heal request: of row `row`, or the newest of the run.
    pub fn heal_snapshot(&self, run: &str, row: Option<u64>) -> Result<Option<String>, CmdError> {
        self.inner.store.heal_snapshot(run, row)
    }

    /// Stop the scheduler and all run drivers and kill their script processes. Rows that
    /// were running stay `running` in the database and are resumed after the next start.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Fire every trigger due in `(from, to]`. The scheduler calls this once a second;
    /// public so tests can drive time themselves. Returns the ids fired.
    pub fn tick_triggers(
        &self,
        from: chrono::DateTime<chrono::Local>,
        to: chrono::DateTime<chrono::Local>,
    ) -> Vec<String> {
        self.inner.fire_due(from, to)
    }
}

impl Inner {
    pub fn ws_url(&self) -> String {
        self.ws_url
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Register the jobs commands. The app must also insert a [`JobsService`] into
/// `registry.extensions()`; without it the commands fail with `INTERNAL`.
pub fn register(registry: &mut Registry) {
    use commands::*;
    registry
        .add::<ScriptList>()
        .add::<ScriptGet>()
        .add::<ScriptSave>()
        .add::<ScriptDelete>()
        .add::<ScriptRun>()
        .add::<ScriptDebugStart>()
        .add::<ScriptDebugCmd>()
        .add::<RunStart>()
        .add::<RunStatusCmd>()
        .add::<RunList>()
        .add::<RunResults>()
        .add::<RunPause>()
        .add::<RunResume>()
        .add::<RunCancel>()
        .add::<RunExport>()
        .add::<RunHealApply>()
        .add::<TriggerSet>()
        .add::<TriggerList>()
        .add::<TriggerRemove>();
}
