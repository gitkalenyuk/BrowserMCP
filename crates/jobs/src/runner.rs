//! Mass runs: "run this script once per data row, N at a time".
//!
//! Each run has one **driver** task while it is running or paused in this process. The driver
//! keeps up to `parallel` **workers** busy; a worker takes one row from the database queue
//! (`Store::claim_next`), runs the script host for it (with retries and backoff) and writes
//! the row result back. After every row the driver emits `run.progress` and decides:
//!
//! * failed row + `stop_on_error` → remaining rows `cancelled`, run `failed`;
//! * row broke on a `page.*` selector error (`NOT_FOUND`, `NOT_INTERACTABLE`, `STALE_REF`,
//!   `TIMEOUT`, `AMBIGUOUS_TARGET`) and `heal` is on → row `needs_heal`, run `paused`, event
//!   `run.heal_needed`. `run.heal_apply` saves the patched script as `<name>.vN`, puts the
//!   `needs_heal` rows back in the queue and resumes with the new version.
//!
//! Pause lets running rows finish and starts no new ones. Cancel kills the running script
//! processes. Everything is in the database, so after a browser restart a run shows up as
//! `paused` and `run.resume` continues it (see `Store::recover_interrupted`).
//!
//! Profiles ([`tab_defaults`]): every row's script host gets `BMCP_TAB_DEFAULTS`, which the
//! host merges into the script's `tabs.open` calls. `profile: "isolated"` gives each row
//! attempt a fresh in-memory profile and off-screen (windowless) tabs, so parallel rows never
//! share cookies, keyboard focus or visibility (a background tab of a window is hidden and
//! ignores input). The run id is always passed, so downloads land in `data/runs/<run>/`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bmcp_core::{CmdError, ErrorCode};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::Inner;
use crate::data::Row;
use crate::host::{HostOutcome, HostRun, ScriptFailure, locate_host, run_host};
use crate::store::{
    ClaimedRow, Counts, HealMode, RowStatus, RowUpdate, RunOptions, RunRecord, RunStatus, now_ms,
};
use crate::topics;

/// Default per-row / per-call script timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Assumed memory per parallel row (script host + its share of browser tabs).
const RAM_PER_SLOT: u64 = 400 * 1024 * 1024;
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Control handle of a live driver.
#[derive(Default)]
pub struct RunCtl {
    /// Poked on pause/resume/cancel/heal so the driver re-reads the run status.
    wake: Notify,
    /// Kills this run's script processes (run.cancel).
    cancel: CancellationToken,
}

/// What to run for `script.run`.
pub enum ScriptSource<'a> {
    /// A saved script.
    Name(&'a str),
    /// Inline code (written to a temporary file next to the saved scripts, so relative
    /// imports work the same).
    Code(&'a str),
}

/// `run.start` arguments after validation.
pub struct StartRequest {
    pub script: String,
    pub rows: Vec<Row>,
    pub parallel: Option<usize>,
    pub retries: Option<u32>,
    pub timeout_secs: Option<u64>,
    pub stop_on_error: bool,
    pub heal: HealMode,
    pub profile: Option<String>,
}

/// `run.start {profile}` value for per-row isolation.
pub const ISOLATED: &str = "isolated";

/// Environment variable the script host reads: JSON merged into `tabs.open` arguments.
pub const TAB_DEFAULTS_ENV: &str = "BMCP_TAB_DEFAULTS";

/// The `tabs.open` defaults of one row attempt (see the module docs).
pub fn tab_defaults(run: &str, row: u64, attempt: u32, profile: Option<&str>) -> Value {
    match profile {
        Some(ISOLATED) => json!({
            // In-memory profile names: a-z 0-9 - _, at most 64 chars.
            "profile": format!("{}-r{row}-a{attempt}", run.to_ascii_lowercase()),
            "incognito": true,
            "window": "offscreen",
            "run": run,
        }),
        Some(p) if p != bmcp_core::ProfileId::DEFAULT => json!({ "profile": p, "run": run }),
        _ => json!({ "run": run }),
    }
}

/// How long the heal hook may take before `run.heal_needed` goes out without a snapshot.
const HEAL_HOOK_TIMEOUT: Duration = Duration::from_secs(20);

/// A row that broke on a selector error and opened a heal request; given to the heal hook.
#[derive(Debug, Clone)]
pub struct HealTrigger {
    pub run: String,
    pub row: u64,
    /// The script (version) the row ran.
    pub script: String,
    pub error: Option<ScriptFailure>,
    pub mode: HealMode,
    /// The `run.heal_needed` event data.
    pub payload: Value,
}

/// Called once per heal request, before `run.heal_needed` is emitted. Returns the text of a
/// page snapshot of the row's tab (stored with [`crate::store::Store::save_heal_snapshot`]).
pub type HealHook =
    Arc<dyn Fn(HealTrigger) -> bmcp_core::BoxFuture<'static, Option<String>> + Send + Sync>;

/// How a worker finished its row.
struct RowEnd {
    idx: u64,
    status: RowStatus,
    error: Option<ScriptFailure>,
    /// The browser is shutting down: nothing was written, the row stays `running`.
    abandoned: bool,
}

impl Inner {
    /// Run one script once and wait for it.
    pub async fn run_script(
        &self,
        source: ScriptSource<'_>,
        inputs: &[(String, String)],
        env: &[(String, String)],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<HostOutcome, CmdError> {
        let exe = locate_host(self.config.host_exe.as_deref())?;
        let (path, temporary) = match source {
            ScriptSource::Name(name) => (self.scripts.require(name)?, false),
            ScriptSource::Code(code) => (self.write_temp_script(code)?, true),
        };
        let ws_url = self.ws_url();
        let outcome = run_host(
            HostRun {
                exe: &exe,
                ws_url: &ws_url,
                script: &path,
                inputs,
                env,
                timeout,
                kill_grace: self.config.kill_grace,
            },
            cancel,
        )
        .await;
        if temporary {
            let _ = std::fs::remove_file(&path);
        }
        Ok(outcome)
    }

    /// `data/scripts/.adhoc-<ms>-<n>.ts` (hidden from `script.list` by the leading dot).
    pub(crate) fn write_temp_script(&self, code: &str) -> Result<PathBuf, CmdError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = self
            .scripts
            .dir()
            .join(format!(".adhoc-{}-{n}.ts", now_ms()));
        std::fs::write(&path, code)
            .map_err(|e| CmdError::internal(format!("{}: {e}", path.display())))?;
        Ok(path)
    }

    /// Create a run and start its driver.
    pub fn start_run(self: &Arc<Self>, req: StartRequest) -> Result<RunRecord, CmdError> {
        self.scripts.require(&req.script)?;
        if let Some(profile) = &req.profile
            && profile != ISOLATED
            && bmcp_core::ProfileId::new(profile.as_str()).is_err()
        {
            return Err(CmdError::new(
                ErrorCode::InvalidParams,
                format!("profile {profile:?} is not a valid profile id"),
            )
            .with_hint("Use \"default\", \"isolated\" (fresh profile per row) or a profile id."));
        }
        let requested = req.parallel.unwrap_or(4).max(1);
        let options = RunOptions {
            parallel: self.effective_parallel(requested),
            parallel_requested: requested,
            retries: req.retries.unwrap_or(1),
            timeout_secs: req.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1),
            stop_on_error: req.stop_on_error,
            heal: req.heal != HealMode::Off,
            heal_mode: Some(req.heal),
            profile: req.profile,
        };
        let run = RunRecord {
            id: self.new_run_id()?,
            script: req.script,
            status: RunStatus::Running,
            options,
            total: req.rows.len() as u64,
            created_ms: now_ms(),
            finished_ms: None,
            heal: None,
            note: None,
        };
        self.store.insert_run(&run, &req.rows)?;
        self.events.emit(
            topics::RUN_STARTED,
            json!({"id": run.id, "script": run.script, "total": run.total,
                   "parallel": run.options.parallel}),
        );
        self.ensure_driver(&run.id);
        Ok(run)
    }

    /// `run-<ms>`, bumped until unique.
    fn new_run_id(&self) -> Result<String, CmdError> {
        let mut ms = now_ms();
        loop {
            let id = format!("run-{ms}");
            if !self.store.run_exists(&id)? {
                return Ok(id);
            }
            ms += 1;
        }
    }

    /// `min(requested, CPUs / 2, free RAM / 400 MB)`, at least 1, at most `max_parallel`.
    fn effective_parallel(&self, requested: usize) -> usize {
        let mut parallel = requested.min(self.config.max_parallel);
        if self.config.auto_limit {
            // The governor's rule (also `governor.recommend_parallel`); if memory cannot be
            // read, only the CPU limit applies.
            parallel = parallel.min(bmcp_governor::recommend_parallel(RAM_PER_SLOT).parallel);
        }
        parallel.max(1)
    }

    /// Start the driver of `id` unless it already has one; wake it either way.
    pub fn ensure_driver(self: &Arc<Self>, id: &str) {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ctl) = runs.get(id) {
            ctl.wake.notify_one();
            return;
        }
        let ctl = Arc::new(RunCtl::default());
        runs.insert(id.to_string(), ctl.clone());
        tokio::spawn(drive(self.clone(), id.to_string(), ctl));
    }

    fn wake(&self, id: &str) {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ctl) = runs.get(id) {
            ctl.wake.notify_one();
        }
    }

    pub fn pause(&self, id: &str) -> Result<(), CmdError> {
        let run = self.store.run(id)?;
        match run.status {
            RunStatus::Running => {
                self.store
                    .set_status(id, RunStatus::Paused, Some("paused by run_pause"))?;
                self.wake(id);
                Ok(())
            }
            RunStatus::Paused => Ok(()),
            status => Err(not_active(id, status)),
        }
    }

    /// Continue a paused run. Rows still waiting for a patch (`needs_heal`) count as failed.
    pub fn resume(self: &Arc<Self>, id: &str) -> Result<(), CmdError> {
        let run = self.store.run(id)?;
        match run.status {
            RunStatus::Paused | RunStatus::Running => {
                self.store
                    .move_rows(id, RowStatus::NeedsHeal, RowStatus::Failed)?;
                self.store.set_heal(id, None)?;
                self.store.set_status(id, RunStatus::Running, None)?;
                self.ensure_driver(id);
                Ok(())
            }
            status => Err(not_active(id, status)),
        }
    }

    /// Stop a run: pending rows become `cancelled`, running script processes are killed.
    /// Cancelling a finished run does nothing.
    pub fn cancel(self: &Arc<Self>, id: &str) -> Result<(), CmdError> {
        let run = self.store.run(id)?;
        if run.status.is_final() {
            return Ok(());
        }
        self.store
            .set_status(id, RunStatus::Cancelled, Some("cancelled by run_cancel"))?;
        self.store
            .move_rows(id, RowStatus::Pending, RowStatus::Cancelled)?;
        self.store
            .move_rows(id, RowStatus::NeedsHeal, RowStatus::Cancelled)?;
        self.store.set_heal(id, None)?;
        if let Some(ctl) = self.runs.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            ctl.cancel.cancel();
        }
        // A paused run after a restart has no driver: start one so it emits run.finished.
        self.ensure_driver(id);
        Ok(())
    }

    /// Save `patched_code` as the next version of the run's script and continue the run with
    /// it, re-queueing the rows that waited for a patch. Returns `(new script, re-queued)`.
    pub fn heal_apply(
        self: &Arc<Self>,
        id: &str,
        patched_code: &str,
    ) -> Result<(String, usize), CmdError> {
        let run = self.store.run(id)?;
        if run.status.is_final() {
            return Err(not_active(id, run.status));
        }
        let script = self.scripts.next_version(&run.script)?;
        self.scripts.save(&script, patched_code)?;
        self.store.set_script(id, &script)?;
        let requeued = self
            .store
            .move_rows(id, RowStatus::NeedsHeal, RowStatus::Pending)?;
        self.store.set_heal(id, None)?;
        self.store.set_status(id, RunStatus::Running, None)?;
        self.ensure_driver(id);
        Ok((script, requeued))
    }

    /// After every finished row: progress event, stop_on_error, heal pause.
    fn after_row(self: &Arc<Self>, id: &str, end: &RowEnd) -> Result<(), CmdError> {
        let run = self.store.run(id)?;
        let counts = self.store.counts(id)?;
        self.events
            .emit(topics::RUN_PROGRESS, progress_json(&run, &counts));

        match end.status {
            RowStatus::Failed if run.options.stop_on_error && run.status == RunStatus::Running => {
                self.store
                    .move_rows(id, RowStatus::Pending, RowStatus::Cancelled)?;
                let reason = end.error.as_ref().map_or("", |e| e.message.as_str());
                let note = format!("stopped: row {} failed ({reason})", end.idx);
                self.store.set_status(id, RunStatus::Failed, Some(&note))?;
            }
            // The first broken row of a pause opens the heal request; later ones just wait.
            RowStatus::NeedsHeal if run.heal.is_none() && !run.status.is_final() => {
                let error = end.error.clone();
                let mode = run.options.heal_mode.unwrap_or(HealMode::Ask);
                let heal = json!({
                    "id": id,
                    "row": end.idx,
                    "script": run.script,
                    "error": error,
                    "step": error.as_ref().and_then(|e| e.command.clone()),
                    "line": error.as_ref().and_then(|e| e.line),
                    "mode": mode.as_str(),
                });
                self.store.set_heal(id, Some(&heal))?;
                self.store.set_status(
                    id,
                    RunStatus::Paused,
                    Some(
                        "waiting for a patched script: call run_heal_apply (or run_resume to skip)",
                    ),
                )?;
                self.announce_heal(HealTrigger {
                    run: id.to_string(),
                    row: end.idx,
                    script: run.script.clone(),
                    error,
                    mode,
                    payload: heal,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Emit `run.heal_needed`. With a snapshot hook installed (see
    /// [`crate::JobsService::set_heal_hook`]) the page snapshot is taken and stored first, so
    /// whoever reacts to the event finds it in the database.
    fn announce_heal(self: &Arc<Self>, trigger: HealTrigger) {
        let hook = self
            .heal_hook
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(hook) = hook else {
            self.events.emit(topics::RUN_HEAL_NEEDED, trigger.payload);
            return;
        };
        let inner = self.clone();
        tokio::spawn(async move {
            // A hanging snapshot must not hold the heal request back for long.
            let snapshot = tokio::time::timeout(HEAL_HOOK_TIMEOUT, hook(trigger.clone()))
                .await
                .ok()
                .flatten();
            if let Some(text) = snapshot
                && let Err(err) = inner
                    .store
                    .save_heal_snapshot(&trigger.run, trigger.row, &text)
            {
                tracing::warn!(run = %trigger.run, %err, "cannot store the heal snapshot");
            }
            inner.events.emit(topics::RUN_HEAL_NEEDED, trigger.payload);
        });
    }

    fn emit_finished(&self, run: &RunRecord) {
        let counts = self.store.counts(&run.id).unwrap_or_default();
        self.events.emit(
            topics::RUN_FINISHED,
            json!({
                "id": run.id,
                "status": run.status,
                "total": run.total,
                "ok": counts.ok,
                "failed": counts.failed,
                "cancelled": counts.cancelled,
                "needs_heal": counts.needs_heal,
                "duration_ms": now_ms().saturating_sub(run.created_ms),
            }),
        );
    }
}

/// The driver loop of one run (see the [module docs](self)). Exits when the run reached a
/// final status (or on shutdown); a paused run keeps its driver waiting for a wake-up.
async fn drive(inner: Arc<Inner>, id: String, ctl: Arc<RunCtl>) {
    let mut workers: JoinSet<RowEnd> = JoinSet::new();
    loop {
        if inner.shutdown.is_cancelled() {
            break; // dropping `workers` aborts them, which kills their processes
        }
        let run = match inner.store.run(&id) {
            Ok(run) => run,
            Err(err) => {
                tracing::error!(run = %id, %err, "run driver stopped");
                break;
            }
        };

        // Fill free worker slots.
        if run.status == RunStatus::Running {
            while workers.len() < run.options.parallel {
                match inner.store.claim_next(&id) {
                    Ok(Some(row)) => {
                        workers.spawn(work_row(
                            inner.clone(),
                            run.clone(),
                            row,
                            ctl.cancel.clone(),
                        ));
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::error!(run = %id, %err, "cannot claim a row");
                        break;
                    }
                }
            }
        }

        // Nothing running: is the run over?
        if workers.is_empty() {
            let counts = inner.store.counts(&id).unwrap_or_default();
            let all_done = counts.pending == 0 && counts.running == 0 && counts.needs_heal == 0;
            if run.status == RunStatus::Running && all_done {
                let _ = inner.store.set_status(&id, RunStatus::Finished, None);
                if let Ok(run) = inner.store.run(&id) {
                    inner.emit_finished(&run);
                }
                break;
            }
            if run.status.is_final() {
                inner.emit_finished(&run);
                break;
            }
        }

        tokio::select! {
            Some(joined) = workers.join_next(), if !workers.is_empty() => {
                match joined {
                    Ok(end) if !end.abandoned => {
                        if let Err(err) = inner.after_row(&id, &end) {
                            tracing::error!(run = %id, %err, "after row");
                        }
                    }
                    Ok(_) => {}
                    Err(err) => tracing::error!(run = %id, %err, "row worker panicked"),
                }
            }
            _ = ctl.wake.notified() => {}
            _ = inner.shutdown.cancelled() => {}
        }
    }
    inner
        .runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
}

/// Run one row: attempts with exponential backoff, then write the final row status.
async fn work_row(
    inner: Arc<Inner>,
    run: RunRecord,
    row: ClaimedRow,
    cancel: CancellationToken,
) -> RowEnd {
    let id = &run.id;
    let timeout = Duration::from_secs(run.options.timeout_secs);
    let started = Instant::now();
    let mut attempts = row.attempts;
    let abandoned = RowEnd {
        idx: row.idx,
        status: RowStatus::Running,
        error: None,
        abandoned: true,
    };
    loop {
        if inner.shutdown.is_cancelled() {
            return abandoned;
        }
        attempts += 1;
        let _ = inner.store.set_attempts(id, row.idx, attempts);
        let defaults = tab_defaults(id, row.idx, attempts, run.options.profile.as_deref());
        let env = [(TAB_DEFAULTS_ENV.to_string(), defaults.to_string())];
        let outcome = inner
            .run_script(
                ScriptSource::Name(&run.script),
                &row.inputs,
                &env,
                timeout,
                &cancel,
            )
            .await;
        if inner.shutdown.is_cancelled() {
            return abandoned;
        }
        // `Err` = could not even start (no host binary, script deleted): retrying won't help.
        let (outcome, retryable) = match outcome {
            Ok(outcome) => (outcome, true),
            Err(err) => (setup_failure(err), false),
        };
        let mut status = if outcome.ok {
            RowStatus::Ok
        } else if cancel.is_cancelled() {
            RowStatus::Cancelled
        } else {
            RowStatus::Failed
        };
        if status == RowStatus::Failed && retryable && attempts <= run.options.retries {
            let backoff = inner
                .config
                .retry_backoff
                .saturating_mul(1 << (attempts - 1).min(16))
                .min(MAX_BACKOFF);
            tokio::select! {
                _ = tokio::time::sleep(backoff) => continue,
                _ = cancel.cancelled() => status = RowStatus::Cancelled,
                _ = inner.shutdown.cancelled() => return abandoned,
            }
        }
        let error = outcome.error.clone();
        if status == RowStatus::Failed
            && run.options.heal
            && error.as_ref().is_some_and(ScriptFailure::needs_heal)
        {
            status = RowStatus::NeedsHeal;
        }
        let update = RowUpdate {
            status,
            attempts,
            result: outcome.result,
            error: error.clone(),
            duration_ms: started.elapsed().as_millis() as u64,
            stdout_tail: outcome.stdout,
            script: run.script.clone(),
        };
        if let Err(err) = inner.store.finish_row(id, row.idx, &update) {
            tracing::error!(run = %id, row = row.idx, %err, "cannot save row result");
        }
        if status == RowStatus::Failed {
            inner.events.emit(
                topics::RUN_ROW_FAILED,
                json!({"id": id, "row": row.idx, "error": error, "attempts": attempts}),
            );
        }
        return RowEnd {
            idx: row.idx,
            status,
            error,
            abandoned: false,
        };
    }
}

/// A host that could not start, as an outcome.
fn setup_failure(err: CmdError) -> HostOutcome {
    HostOutcome {
        ok: false,
        exit_code: None,
        duration_ms: 0,
        result: Value::Null,
        stdout: String::new(),
        stderr: err.message.clone(),
        error: Some(ScriptFailure {
            code: "HOST_ERROR".into(),
            command: None,
            message: err.message,
            line: None,
        }),
    }
}

pub fn progress_json(run: &RunRecord, counts: &Counts) -> Value {
    json!({
        "id": run.id,
        "done": counts.done(),
        "ok": counts.ok,
        "failed": counts.failed,
        "needs_heal": counts.needs_heal,
        "cancelled": counts.cancelled,
        "running": counts.running,
        "pending": counts.pending,
        "total": run.total,
    })
}

fn not_active(id: &str, status: RunStatus) -> CmdError {
    CmdError::invalid_params(format!("run '{id}' is {}", status.as_str()))
        .with_hint("Only running or paused runs can be changed; start a new run with run_start.")
}
