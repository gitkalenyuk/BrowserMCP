//! BrowserMCP built-in AI (plan: "Вбудований ШІ", "Кілька ШІ", Phase 5).
//!
//! * [`config`] — provider presets + `data/config/ai.json` (plain JSON, keys included);
//! * [`model`] — HTTP adapters: Anthropic Messages, OpenAI-compatible chat.completions, Gemini;
//! * [`agent`] — the agent loop for API providers; tools are the registry's commands;
//! * [`cli`] — Claude Code (`claude -p … stream-json`) and Codex (`codex app-server`) sessions;
//! * [`session`] — agent id, color, status, transcript, touched tabs, `ai.*` events;
//! * [`term`] — PTY terminals (`term.*`).
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
        let mut config = lock(&self.config);
        f(&mut config);
        config
            .save(&self.config_path())
            .map_err(|e| CmdError::internal(format!("cannot save ai.json: {e}")))
    }

    /// A provider preset merged with its settings.
    pub fn resolve(&self, id: &str) -> Result<Resolved, CmdError> {
        let preset = config::preset(id).ok_or_else(|| {
            let ids = config::PRESETS
                .iter()
                .map(|p| serde_json::json!(p.id))
                .collect();
            CmdError::new(ErrorCode::NotFound, format!("unknown AI provider '{id}'"))
                .with_hint("Call ai_providers to list the providers.")
                .with_candidates(ids)
        })?;
        Ok(Resolved::new(preset, lock(&self.config).settings(id)))
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
        if !resolved.configured() {
            return Err(CmdError::invalid_params(format!(
                "AI provider '{}' is not configured",
                opts.provider
            ))
            .with_hint(match resolved.preset.kind {
                Kind::Api(_) => "Set its API key (and base_url for custom) with ai_provider_set.",
                Kind::ClaudeCode => {
                    "Install Claude Code (`claude` on PATH) or set its path with ai_provider_set."
                }
                Kind::Codex => {
                    "Install Codex (`codex` on PATH) or set its path with ai_provider_set."
                }
            }));
        }
        let model = opts
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .or(resolved.model.clone());

        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("{}-{n}", opts.provider);
        let color = COLORS[(n as usize - 1) % COLORS.len()].to_string();
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

        match resolved.preset.kind {
            Kind::Api(api) => {
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
                    client: model::Client::new(
                        api,
                        resolved.base_url.clone(),
                        resolved.api_key.clone(),
                    ),
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
                if resolved.preset.kind == Kind::ClaudeCode {
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
