//! BrowserMCP built-in AI (plan: "Вбудований ШІ", "Кілька ШІ", Phase 5).
//!
//! * [`config`] — built-in presets, the user's own providers and `data/config/ai.json` (plain
//!   JSON, keys included);
//! * [`model`] — HTTP adapters: Anthropic Messages, OpenAI-compatible chat.completions, Gemini;
//! * [`agent`] — the agent loop for API providers; tools are the registry's commands;
//! * [`cli`] — Claude Code (`claude -p … stream-json`) and Codex (`codex app-server`) sessions;
//! * [`session`] — agent id, color, status, transcript, touched tabs, `ai.*` events;
//! * [`term`] — PTY terminals (`term.*`);
//! * [`proc_tree`] — kill a child with everything it started (Windows Job Objects);
//! * [`oneshot`] — one-shot text completions (no tools) for self-healing and `page.extract`.
//!
//! Wiring (in `crates/app`):
//!
//! ```ignore
//! registry.extensions().insert(bmcp_ai::AiService::new(data.root(), mcp_url));
//! bmcp_ai::register(&mut registry);
//! let registry = Arc::new(registry);
//! bmcp_ai::attach(&registry); // agents call tools through the shared registry
//! ```

pub mod agent;
pub mod cli;
pub mod commands;
pub mod config;
pub mod model;
pub mod oneshot;
pub mod proc_tree;
pub mod session;
pub mod term;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use bmcp_core::{CmdError, Ctx, ErrorCode, Registry, Toolset};

use crate::config::{AiConfig, Kind, Resolved};
use crate::session::{COLORS, Session, SessionInfo};

pub use commands::register;

/// Shared state of the AI features, stored in the registry's `Extensions`.
pub struct AiService {
    data: PathBuf,
    /// The browser's MCP endpoint, e.g. `http://127.0.0.1:47800/mcp` (for CLI agents).
    mcp_url: Mutex<String>,
    /// Set by [`attach`]; weak because the registry owns this service.
    registry: OnceLock<Weak<Registry>>,
    config: Mutex<AiConfig>,
    sessions: Mutex<BTreeMap<String, Arc<Session>>>,
    counter: AtomicU64,
    pub terminals: term::Terminals,
}

/// Give the service its registry (call once the registry is in an `Arc`). Without it API
/// agents cannot run tools.
pub fn attach(registry: &Arc<Registry>) {
    if let Some(service) = registry.extensions().get::<AiService>() {
        let _ = service.registry.set(Arc::downgrade(registry));
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Options for [`AiService::start_session`].
#[derive(Debug, Clone)]
pub struct StartOptions {
    pub provider: String,
    pub model: Option<String>,
    pub prompt: String,
    pub toolsets: Vec<Toolset>,
    pub max_steps: u32,
}

impl AiService {
    /// `data_root` is the portable `data/` folder; `mcp_url` the browser's MCP endpoint.
    pub fn new(data_root: impl Into<PathBuf>, mcp_url: impl Into<String>) -> Self {
        let data = data_root.into();
        let config = AiConfig::load(&data.join("config").join("ai.json"));
        AiService {
            data,
            mcp_url: Mutex::new(mcp_url.into()),
            registry: OnceLock::new(),
            config: Mutex::new(config),
            sessions: Mutex::default(),
            counter: AtomicU64::new(0),
            terminals: term::Terminals::default(),
        }
    }

    /// The service from a command's context.
    pub fn from_ctx(ctx: &Ctx) -> Result<Arc<AiService>, CmdError> {
        ctx.extensions.get::<AiService>().ok_or_else(|| {
            CmdError::internal("the AI service is not set up in this build")
                .with_hint("The app must insert bmcp_ai::AiService into the registry extensions.")
        })
    }

    /// Change the MCP endpoint (e.g. once the real port is known).
    pub fn set_mcp_url(&self, url: impl Into<String>) {
        *lock(&self.mcp_url) = url.into();
    }

    fn config_path(&self) -> PathBuf {
        self.data.join("config").join("ai.json")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.data.join("ai").join("sessions")
    }

    /// Working directory of CLI agents.
    fn workspace_dir(&self) -> PathBuf {
        self.data.join("ai").join("workspace")
    }

    pub fn config(&self) -> AiConfig {
        lock(&self.config).clone()
    }

    /// Change the config with `f` and save it.
    pub fn update_config(&self, f: impl FnOnce(&mut AiConfig)) -> Result<(), CmdError> {
        self.try_update_config(|config| {
            f(config);
            Ok(())
        })
    }

    /// Change the config with `f` and save it; when `f` (or saving) fails nothing changes.
    pub fn try_update_config<T>(
        &self,
        f: impl FnOnce(&mut AiConfig) -> Result<T, CmdError>,
    ) -> Result<T, CmdError> {
        let mut config = lock(&self.config);
        let mut next = config.clone();
        let out = f(&mut next)?;
        next.save(&self.config_path())
            .map_err(|e| CmdError::internal(format!("cannot save ai.json: {e}")))?;
        *config = next;
        Ok(out)
    }

    /// A built-in or user provider merged with its settings.
    pub fn resolve(&self, id: &str) -> Result<Resolved, CmdError> {
        // Resolving may search PATH for a CLI: do it on a copy, not under the lock.
        self.config().resolve(id)
    }

    pub fn session(&self, id: &str) -> Result<Arc<Session>, CmdError> {
        lock(&self.sessions).get(id).cloned().ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, format!("no AI session '{id}'"))
                .with_hint("Call ai_sessions_list to see the sessions.")
        })
    }

    pub fn sessions(&self) -> Vec<SessionInfo> {
        lock(&self.sessions).values().map(|s| s.info()).collect()
    }

    /// Start an agent and queue `prompt` as its first message. Needs a tokio runtime.
    pub fn start_session(&self, ctx: &Ctx, opts: StartOptions) -> Result<SessionInfo, CmdError> {
        let resolved = self.resolve(&opts.provider)?;
        resolved.ensure_configured()?;
        let model = opts
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .or(resolved.model.clone());

        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("{}-{n}", opts.provider);
        // The agent registry hands out the tab-group color, so the chat and the tab group
        // match; its named colors map onto `COLORS` in the same order.
        let color = match ctx.extensions.get::<bmcp_core::Agents>() {
            Some(agents) => {
                let name = format!("{} {n}", resolved.name);
                let named = agents.ensure(&ctx.events, &id, Some(&name)).color;
                let index = bmcp_core::agents::COLORS
                    .iter()
                    .position(|c| *c == named)
                    .unwrap_or(0);
                COLORS[index % COLORS.len()].to_string()
            }
            None => COLORS[(n as usize - 1) % COLORS.len()].to_string(),
        };
        let dir = self.sessions_dir();
        let log = std::fs::create_dir_all(&dir)
            .ok()
            .map(|_| dir.join(format!("{id}.jsonl")));
        let (session, input) = Session::new(
            id.clone(),
            opts.provider.clone(),
            model.clone(),
            color,
            ctx.events.clone(),
            log,
        );
        let session = Arc::new(session);

        match resolved.kind {
            Kind::Api(_) => {
                let model = model.ok_or_else(|| {
                    CmdError::invalid_params(format!("no model set for '{}'", opts.provider)).with_hint(
                        "Pass `model`, or set one with ai_provider_set (ai_provider_test lists the models).",
                    )
                })?;
                let registry = self.registry.get().cloned().unwrap_or_default();
                let tools = registry
                    .upgrade()
                    .map(|r| agent::tool_defs(&r, &opts.toolsets))
                    .unwrap_or_default();
                let agent = agent::ApiAgent {
                    session: session.clone(),
                    client: resolved.client().expect("an API provider"),
                    model,
                    tools,
                    runner: agent::ToolRunner {
                        registry,
                        backend: ctx.backend.clone(),
                        events: ctx.events.clone(),
                        agent: id.clone(),
                    },
                    max_steps: opts.max_steps,
                };
                tokio::spawn(agent.run(input));
            }
            Kind::ClaudeCode | Kind::Codex => {
                let workspace = self.workspace_dir();
                std::fs::create_dir_all(&workspace).map_err(|e| {
                    CmdError::internal(format!("cannot create {}: {e}", workspace.display()))
                })?;
                let toolsets: Vec<&str> = opts.toolsets.iter().map(|t| t.as_str()).collect();
                let mcp_url = format!(
                    "{}?agent={id}&toolsets={}",
                    lock(&self.mcp_url),
                    toolsets.join(",")
                );
                let binary = resolved.binary.clone().unwrap_or_default();
                if resolved.kind == Kind::ClaudeCode {
                    let agent = cli::claude::ClaudeCode {
                        session: session.clone(),
                        binary,
                        model,
                        mcp_url,
                        mcp_config: self.sessions_dir().join(format!("{id}.mcp.json")),
                        cwd: workspace,
                    };
                    tokio::spawn(agent.run(input));
                } else {
                    let agent = cli::codex::Codex {
                        session: session.clone(),
                        binary,
                        model,
                        mcp_url,
                        cwd: workspace,
                    };
                    tokio::spawn(agent.run(input));
                }
            }
        }

        lock(&self.sessions).insert(id, session.clone());
        session.send(opts.prompt).map_err(CmdError::internal)?;
        Ok(session.info())
    }

    /// Stop every session and terminal (browser shutdown).
    pub fn shutdown(&self) {
        for session in lock(&self.sessions).values() {
            session.cancel.cancel();
        }
        self.terminals.close_all();
    }
}

/// `["core", "net"]` → toolsets; empty → `[core]`.
pub fn parse_toolsets(names: &[String]) -> Result<Vec<Toolset>, CmdError> {
    if names.is_empty() {
        return Ok(vec![Toolset::Core]);
    }
    names.iter().map(|n| Toolset::from_str(n)).collect()
}

impl Drop for AiService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
