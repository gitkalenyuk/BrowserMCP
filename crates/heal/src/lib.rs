//! BrowserMCP self-healing and AI-built extraction (plan: "Поломки: self-healing" and
//! "Витягування даних: extract(schema)").
//!
//! **Self-healing.** `crates/jobs` pauses a run when a row breaks on a `page.*` selector error
//! and emits `run.heal_needed`. This crate closes the loop:
//!
//! ```text
//! row breaks ─► jobs calls our snapshot hook (page snapshot of the row's tab → runs DB)
//!            ─► run.heal_needed ─► watcher: run.heal_propose (AI writes a minimal patch,
//!               validated: applies, stays near the failing line, transpiles)
//!            ─► mode "ask":  run.heal_proposed {id, proposal_id, diff, explanation, …}
//!                            a person answers run.heal_accept / run.heal_reject {feedback}
//!               mode "auto": confidence >= 0.8 and a dry run of the failing row passes →
//!                            applied at once (run.heal_applied {auto: true}); else like "ask"
//!            ─► run.heal_accept → run.heal_apply: patched script saved as <name>.vN, the
//!               broken rows re-queued, the rest of the run uses the patch.
//! ```
//!
//! **Extraction.** `page.extract {schema}`: the AI builds a selector plan once, the plan is
//! cached in `data/extract-cache/<key>.json` and later calls run without AI (see [`extract`]).
//!
//! Modules: [`heal`] (proposals), [`patch`] (answer parsing, diff applying, checks),
//! [`context`] (the prompt), [`watch`] (events, tab tracking, snapshot hook), [`extract`],
//! [`commands`]. Event shapes are documented in `crates/heal/README.md`.
//!
//! Wiring (in `crates/app`, after the registry is in an `Arc`):
//!
//! ```ignore
//! registry.extensions().insert(bmcp_heal::HealService::new(bmcp_heal::HealConfig::new(&data)));
//! bmcp_heal::register(&mut registry);
//! let registry = Arc::new(registry);
//! bmcp_heal::attach(&registry, backend, events); // starts the background watcher
//! ```

pub mod commands;
pub mod context;
pub mod extract;
pub mod heal;
pub mod patch;
pub mod watch;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use bmcp_ai::oneshot::{Completion, TextRequest};
use bmcp_core::{
    BoxFuture, BrowserBackend, Caller, CmdError, Ctx, DataDir, ErrorCode, EventBus, Registry,
};
use serde_json::Value;

pub use commands::register;

/// Events this crate emits (in addition to jobs' `run.*`).
pub mod topics {
    /// `{id, proposal_id, row, script, diff, explanation, confidence, attempt, max_attempts,
    /// checks, mode, note?}`: a validated patch waits for `run.heal_accept` / `run.heal_reject`.
    pub const HEAL_PROPOSED: &str = "run.heal_proposed";
    /// `{id, proposal_id, script, requeued, auto}`: a patch was applied; the run continues.
    pub const HEAL_APPLIED: &str = "run.heal_applied";
    /// `{id, row, attempts, error}`: no usable patch (attempts used up, or no AI provider).
    /// The run stays paused: fix the script by hand (`run.heal_apply`) or `run.resume`.
    pub const HEAL_FAILED: &str = "run.heal_failed";
}

/// Knobs of the heal service.
#[derive(Debug, Clone)]
pub struct HealConfig {
    /// `data/heal` (temporary files of the transpile check).
    pub heal_dir: PathBuf,
    /// `data/extract-cache` (cached extraction plans).
    pub extract_cache_dir: PathBuf,
    /// `bmcp-script-host` for the transpile check; `None` = search like the jobs crate does.
    pub host_exe: Option<PathBuf>,
    /// AI attempts per heal request (validation failures and rejections count).
    pub max_attempts: u32,
    /// "auto" mode applies only proposals at least this confident.
    pub auto_threshold: f64,
    /// Lines around the failing line a patch may change without a justification.
    pub radius: usize,
    /// "auto" mode applies at most this many patches per run, then asks.
    pub max_auto_per_run: u32,
}

impl HealConfig {
    /// The standard layout inside the portable data folder.
    pub fn new(data: &DataDir) -> Self {
        Self::in_dir(data.root())
    }

    /// Same layout under any folder (tests).
    pub fn in_dir(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        HealConfig {
            heal_dir: root.join("heal"),
            extract_cache_dir: root.join("extract-cache"),
            host_exe: None,
            max_attempts: 3,
            auto_threshold: 0.8,
            radius: 15,
            max_auto_per_run: 3,
        }
    }
}

/// Where AI completions come from. The default is the configured provider through
/// [`bmcp_ai::AiService`]; tests plug in canned answers.
pub trait Llm: Send + Sync {
    fn complete(&self, req: TextRequest) -> BoxFuture<'static, Result<Completion, CmdError>>;
}

/// What [`attach`] hands over.
struct Links {
    registry: Weak<Registry>,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
}

/// The long-lived state of self-healing and extraction; lives in the registry's `Extensions`.
pub struct HealService {
    pub config: HealConfig,
    llm: Option<Arc<dyn Llm>>,
    links: OnceLock<Links>,
    state: Mutex<heal::State>,
    /// One proposal at a time (they share sessions and cost AI calls).
    propose_lock: tokio::sync::Mutex<()>,
}

impl HealService {
    pub fn new(config: HealConfig) -> Self {
        HealService {
            config,
            llm: None,
            links: OnceLock::new(),
            state: Mutex::default(),
            propose_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Use `llm` instead of the configured AI provider (tests).
    pub fn with_llm(mut self, llm: Arc<dyn Llm>) -> Self {
        self.llm = Some(llm);
        self
    }

    pub fn from_ctx(ctx: &Ctx) -> Result<Arc<HealService>, CmdError> {
        ctx.extensions.get::<HealService>().ok_or_else(|| {
            CmdError::internal("the heal service is not set up in this build")
                .with_hint("The app must insert bmcp_heal::HealService and call bmcp_heal::attach.")
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, heal::State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn links(&self) -> Result<&Links, CmdError> {
        self.links
            .get()
            .ok_or_else(|| CmdError::internal("bmcp_heal::attach was not called"))
    }

    fn registry(&self) -> Result<Arc<Registry>, CmdError> {
        self.links()?
            .registry
            .upgrade()
            .ok_or_else(|| CmdError::new(ErrorCode::ShuttingDown, "the browser is shutting down"))
    }

    /// Run a registry command as `caller`.
    async fn call_as(&self, caller: Caller, name: &str, args: Value) -> Result<Value, CmdError> {
        let registry = self.registry()?;
        let links = self.links()?;
        let ctx = Ctx::new(caller, links.backend.clone(), links.events.clone());
        registry.call(ctx, name, args).await
    }

    /// Run a registry command as the heal service itself.
    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        let caller = Caller::Agent { id: "heal".into() };
        self.call_as(caller, name, args).await
    }

    fn emit(&self, topic: &str, data: Value) {
        if let Ok(links) = self.links() {
            links.events.emit(topic, data);
        }
    }

    /// One AI completion: the test double, else the configured provider.
    async fn complete(&self, req: TextRequest) -> Result<Completion, CmdError> {
        if let Some(llm) = &self.llm {
            return llm.complete(req).await;
        }
        let ai = self
            .registry()?
            .extensions()
            .get::<bmcp_ai::AiService>()
            .ok_or_else(|| CmdError::internal("the AI service is not set up in this build"))?;
        ai.complete_text(req).await
    }

    /// `bmcp-script-host` for the transpile check, when it can be found.
    fn host(&self) -> Option<PathBuf> {
        match &self.config.host_exe {
            Some(path) => Some(path.clone()),
            None => bmcp_jobs::host::locate_host(None).ok(),
        }
    }

    fn jobs(&self) -> Option<bmcp_jobs::JobsService> {
        self.registry()
            .ok()?
            .extensions()
            .get::<bmcp_jobs::JobsService>()
            .map(|j| (*j).clone())
    }
}

/// Connect the service (in the registry's extensions) to the shared registry, the browser and
/// the event bus; install the jobs snapshot hook and start the background watcher. Call once,
/// inside a tokio runtime, after the registry is in an `Arc`.
pub fn attach(registry: &Arc<Registry>, backend: Arc<dyn BrowserBackend>, events: EventBus) {
    let Some(heal) = registry.extensions().get::<HealService>() else {
        tracing::warn!("bmcp_heal::attach: no HealService in the registry");
        return;
    };
    let rx = events.subscribe();
    let _ = heal.links.set(Links {
        registry: Arc::downgrade(registry),
        backend,
        events,
    });
    if let Some(jobs) = registry.extensions().get::<bmcp_jobs::JobsService>() {
        let hook_heal = heal.clone();
        jobs.set_heal_hook(Arc::new(move |trigger| {
            let heal = hook_heal.clone();
            Box::pin(async move { heal.snapshot_for(&trigger).await })
        }));
    }
    tokio::spawn(watch::watcher(Arc::downgrade(&heal), rx));
}
