//! `ai.*` and `term.*` commands (toolset `ai`).

use std::path::PathBuf;
use std::time::Duration;

use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, Registry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Kind, PRESETS, Resolved};
use crate::model::Api;
use crate::session::SessionInfo;
use crate::term::{OpenOptions, TermInfo};
use crate::{AiService, StartOptions, parse_toolsets};

/// Register every command of this crate. The [`AiService`] itself is inserted by the app
/// (it needs the data folder and the MCP URL).
pub fn register(registry: &mut Registry) {
    registry
        .add::<Providers>()
        .add::<ProviderSet>()
        .add::<ProviderTest>()
        .add::<SessionStart>()
        .add::<SessionSend>()
        .add::<SessionStop>()
        .add::<SessionsList>()
        .add::<SessionTranscript>()
        .add::<TermOpen>()
        .add::<TermWrite>()
        .add::<TermResize>()
        .add::<TermClose>()
        .add::<TermList>();
}

fn not_found(message: String) -> CmdError {
    CmdError::new(ErrorCode::NotFound, message)
}

// ---------------------------------------------------------------- providers

/// One provider as listed by `ai.providers`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProviderInfo {
    /// e.g. `anthropic`, `openai`, `ollama`, `claude-code`.
    pub id: String,
    pub name: String,
    /// `api` (BrowserMCP runs the agent loop) or `cli` (Claude Code / Codex).
    pub kind: String,
    /// Wire protocol: `anthropic`, `openai`, `gemini`, `claude-code`, `codex`.
    pub protocol: String,
    /// Ready to start a session (key set / CLI installed).
    pub configured: bool,
    pub needs_key: bool,
    /// Whether an API key is stored (the key itself is not returned).
    pub has_key: bool,
    pub base_url: String,
    /// Model used when `ai.session_start` gives none.
    pub model: Option<String>,
    /// Known models (live list after `ai.provider_test`).
    pub models: Vec<String>,
    /// CLI providers: the binary that will run.
    pub path: Option<String>,
    /// Used by background AI features (self-healing, page.extract) when they name no provider.
    pub is_default: bool,
}

fn provider_info(r: &Resolved, default: Option<&str>) -> ProviderInfo {
    let (kind, protocol) = match r.preset.kind {
        Kind::Api(Api::Anthropic) => ("api", "anthropic"),
        Kind::Api(Api::OpenAi) => ("api", "openai"),
        Kind::Api(Api::Gemini) => ("api", "gemini"),
        Kind::ClaudeCode => ("cli", "claude-code"),
        Kind::Codex => ("cli", "codex"),
    };
    ProviderInfo {
        id: r.preset.id.into(),
        name: r.preset.name.into(),
        kind: kind.into(),
        protocol: protocol.into(),
        configured: r.configured(),
        needs_key: r.preset.needs_key,
        has_key: r.api_key.is_some(),
        base_url: r.base_url.clone(),
        model: r.model.clone(),
        models: r.models.clone(),
        path: r.binary.as_ref().map(|p| p.to_string_lossy().into_owned()),
        is_default: default == Some(r.preset.id),
    }
}

/// `ai.providers`
pub struct Providers;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProvidersOutput {
    pub providers: Vec<ProviderInfo>,
}

impl Command for Providers {
    const NAME: &'static str = "ai.providers";
    type Args = Empty;
    type Output = ProvidersOutput;

    fn meta() -> Meta {
        Meta::new(
            "List AI providers (Anthropic, OpenAI-compatible, Gemini, Ollama, LM Studio, Claude Code, \
             Codex…) with configured flag, base URL, default model and known models.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ProvidersOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let default = ai.default_provider();
        let providers = PRESETS
            .iter()
            .map(|p| {
                ai.resolve(p.id)
                    .map(|r| provider_info(&r, default.as_deref()))
            })
            .collect::<Result<_, _>>()?;
        Ok(ProvidersOutput { providers })
    }
}

/// `ai.provider_set`
pub struct ProviderSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSetArgs {
    /// Provider id from ai.providers.
    pub id: String,
    /// API key; "" removes it. Stored in plain JSON in data/config/ai.json.
    #[serde(default)]
    pub api_key: Option<String>,
    /// API root, e.g. "http://localhost:11434/v1"; "" = preset default.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Default model; "" = preset default.
    #[serde(default)]
    pub model: Option<String>,
    /// CLI providers: path of the claude / codex binary; "" = search PATH.
    #[serde(default)]
    pub path: Option<String>,
    /// true: make this the default provider of background AI features (self-healing,
    /// page.extract); false: stop it being the default.
    #[serde(default)]
    pub default: Option<bool>,
}

impl Command for ProviderSet {
    const NAME: &'static str = "ai.provider_set";
    type Args = ProviderSetArgs;
    type Output = ProviderInfo;

    fn meta() -> Meta {
        Meta::new(
            "Set a provider's API key, base URL, default model or CLI path (saved to \
             data/config/ai.json). Omitted fields stay unchanged; \"\" clears one. Returns the provider.",
            Toolset::Ai,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: ProviderSetArgs) -> Result<ProviderInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        ai.resolve(&args.id)?; // unknown id → NOT_FOUND
        // Some("") clears, None keeps.
        fn apply(slot: &mut Option<String>, value: Option<String>) {
            if let Some(value) = value {
                *slot = Some(value.trim().to_string()).filter(|v| !v.is_empty());
            }
        }
        ai.update_config(|config| {
            let settings = config.providers.entry(args.id.clone()).or_default();
            apply(&mut settings.api_key, args.api_key);
            apply(&mut settings.base_url, args.base_url);
            apply(&mut settings.model, args.model);
            apply(&mut settings.path, args.path);
            match args.default {
                Some(true) => config.default_provider = Some(args.id.clone()),
                Some(false) if config.default_provider.as_deref() == Some(args.id.as_str()) => {
                    config.default_provider = None
                }
                _ => {}
            }
        })?;
        let default = ai.default_provider();
        Ok(provider_info(&ai.resolve(&args.id)?, default.as_deref()))
    }
}

/// `ai.provider_test`
pub struct ProviderTest;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderIdArgs {
    /// Provider id from ai.providers.
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProviderTestOutput {
    pub id: String,
    pub ok: bool,
    /// API providers: models offered by the endpoint (also saved for ai.providers).
    pub models: Vec<String>,
    /// CLI providers: output of `--version`.
    pub version: Option<String>,
    pub error: Option<String>,
}

impl Command for ProviderTest {
    const NAME: &'static str = "ai.provider_test";
    type Args = ProviderIdArgs;
    type Output = ProviderTestOutput;

    fn meta() -> Meta {
        Meta::new(
            "Check a provider without spending tokens: API providers list their models \
             (GET /models), CLI providers run --version. Returns ok, models/version or error.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, args: ProviderIdArgs) -> Result<ProviderTestOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let r = ai.resolve(&args.id)?;
        let mut out = ProviderTestOutput {
            id: args.id.clone(),
            ok: false,
            models: Vec::new(),
            version: None,
            error: None,
        };
        match r.preset.kind {
            Kind::Api(api) => {
                if r.base_url.is_empty() {
                    out.error = Some("no base_url set".into());
                    return Ok(out);
                }
                let client = crate::model::Client::new(api, r.base_url.clone(), r.api_key.clone());
                match client.list_models().await {
                    Ok(models) => {
                        out.ok = true;
                        if !models.is_empty() {
                            let saved = models.clone();
                            ai.update_config(|c| {
                                c.providers.entry(args.id.clone()).or_default().models = saved
                            })?;
                        }
                        out.models = models;
                    }
                    Err(err) => out.error = Some(err.to_string()),
                }
            }
            Kind::ClaudeCode | Kind::Codex => match &r.binary {
                None => out.error = Some(format!("{} not found", r.preset.name)),
                Some(binary) => match cli_version(binary).await {
                    Ok(version) => {
                        out.ok = true;
                        out.version = Some(version);
                    }
                    Err(err) => out.error = Some(err),
                },
            },
        }
        Ok(out)
    }
}

async fn cli_version(binary: &std::path::Path) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.arg("--version")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = tokio::time::timeout(Duration::from_secs(20), cmd.output())
        .await
        .map_err(|_| "--version timed out".to_string())?
        .map_err(|e| format!("cannot run {}: {e}", binary.display()))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---------------------------------------------------------------- sessions

/// `ai.session_start`
pub struct SessionStart;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionStartArgs {
    /// Provider id from ai.providers, e.g. "anthropic", "ollama", "claude-code".
    pub provider: String,
    /// Model; default = the provider's configured model.
    #[serde(default)]
    pub model: Option<String>,
    /// First user message.
    pub prompt: String,
    /// Toolsets the agent may use (default ["core"]).
    #[serde(default)]
    pub toolsets: Vec<String>,
    /// API providers: most model calls per user message (default 30).
    #[serde(default)]
    #[schemars(range(min = 1, max = 200))]
    pub max_steps: Option<u32>,
}

impl Command for SessionStart {
    const NAME: &'static str = "ai.session_start";
    type Args = SessionStartArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Start an AI agent session that drives the browser through BrowserMCP's tools. Progress \
             arrives as ai.message / ai.tool_call / ai.tool_result / ai.done events. Returns the \
             session (id = agent id, color, status).",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionStartArgs) -> Result<SessionInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        if args.prompt.trim().is_empty() {
            return Err(CmdError::invalid_params("prompt is empty"));
        }
        let opts = StartOptions {
            provider: args.provider,
            model: args.model,
            prompt: args.prompt,
            toolsets: parse_toolsets(&args.toolsets)?,
            max_steps: args.max_steps.unwrap_or(30).clamp(1, 200),
        };
        ai.start_session(&ctx, opts)
    }
}

/// `ai.session_send`
pub struct SessionSend;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSendArgs {
    /// Session id from ai.session_start / ai.sessions_list.
    pub id: String,
    /// Next user message.
    pub text: String,
}

impl Command for SessionSend {
    const NAME: &'static str = "ai.session_send";
    type Args = SessionSendArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Send the next user message to an AI session (queued if it is still working). Returns the session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionSendArgs) -> Result<SessionInfo, CmdError> {
        let session = AiService::from_ctx(&ctx)?.session(&args.id)?;
        session.send(args.text).map_err(|m| {
            CmdError::invalid_params(m).with_hint("Start a new session with ai_session_start.")
        })?;
        Ok(session.info())
    }
}

/// `ai.session_stop`
pub struct SessionStop;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionIdArgs {
    /// Session id.
    pub id: String,
}

impl Command for SessionStop {
    const NAME: &'static str = "ai.session_stop";
    type Args = SessionIdArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Stop an AI session: cancels the running model call / kills the CLI agent. Returns the session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionIdArgs) -> Result<SessionInfo, CmdError> {
        let session = AiService::from_ctx(&ctx)?.session(&args.id)?;
        session.cancel.cancel();
        // Give the task a moment to report `stopped`.
        for _ in 0..50 {
            if matches!(
                session.status(),
                crate::session::Status::Stopped | crate::session::Status::Failed
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(session.info())
    }
}

/// `ai.sessions_list`
pub struct SessionsList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionsListOutput {
    pub sessions: Vec<SessionInfo>,
}

impl Command for SessionsList {
    const NAME: &'static str = "ai.sessions_list";
    type Args = Empty;
    type Output = SessionsListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List AI sessions (agents) with id, provider, color, status and the tabs each touched.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<SessionsListOutput, CmdError> {
        Ok(SessionsListOutput {
            sessions: AiService::from_ctx(&ctx)?.sessions(),
        })
    }
}

/// `ai.session_transcript`
pub struct SessionTranscript;

#[derive(Debug, Serialize, JsonSchema)]
pub struct TranscriptOutput {
    pub id: String,
    /// Entries `{type: user|assistant|tool_call|tool_result|notice|done, ts_ms, …}`.
    pub entries: Vec<Value>,
}

impl Command for SessionTranscript {
    const NAME: &'static str = "ai.session_transcript";
    type Args = SessionIdArgs;
    type Output = TranscriptOutput;

    fn meta() -> Meta {
        Meta::new(
            "The transcript of an AI session: user and assistant messages, tool calls and results.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, args: SessionIdArgs) -> Result<TranscriptOutput, CmdError> {
        let session = AiService::from_ctx(&ctx)?.session(&args.id)?;
        Ok(TranscriptOutput {
            id: args.id,
            entries: session.transcript(),
        })
    }
}

// ---------------------------------------------------------------- terminals

/// `term.open`
pub struct TermOpen;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TermOpenArgs {
    /// "pwsh", "cmd", "bash" or a program path; default pwsh (Windows) / $SHELL.
    #[serde(default)]
    pub shell: Option<String>,
    /// Extra arguments for the shell.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory; default the user's home.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_cols")]
    #[schemars(range(min = 1, max = 1000))]
    pub cols: u16,
    #[serde(default = "default_rows")]
    #[schemars(range(min = 1, max = 1000))]
    pub rows: u16,
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

impl Command for TermOpen {
    const NAME: &'static str = "term.open";
    type Args = TermOpenArgs;
    type Output = TermInfo;

    fn meta() -> Meta {
        Meta::new(
            "Open a terminal (PTY shell). Output arrives as term.output {id, data} events, \
             term.exit {id, code} when it ends. Returns the terminal id.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: TermOpenArgs) -> Result<TermInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let opts = OpenOptions {
            shell: args.shell,
            args: args.args,
            cwd: args.cwd.filter(|c| !c.is_empty()).map(PathBuf::from),
            cols: args.cols,
            rows: args.rows,
        };
        ai.terminals
            .open(ctx.events.clone(), opts)
            .map_err(CmdError::internal)
    }
}

/// `term.write`
pub struct TermWrite;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TermWriteArgs {
    /// Terminal id from term.open.
    pub id: String,
    /// Text/keystrokes to send, e.g. "dir\r".
    pub data: String,
}

impl Command for TermWrite {
    const NAME: &'static str = "term.write";
    type Args = TermWriteArgs;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new(
            "Send input (text, control characters) to a terminal.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: TermWriteArgs) -> Result<Empty, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        // Pipe writes can block when the shell is not reading: keep them off the async threads.
        tokio::task::spawn_blocking(move || ai.terminals.write(&args.id, &args.data))
            .await
            .map_err(|e| CmdError::internal(e.to_string()))?
            .map_err(not_found)?;
        Ok(Empty {})
    }
}

/// `term.resize`
pub struct TermResize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TermResizeArgs {
    pub id: String,
    #[schemars(range(min = 1, max = 1000))]
    pub cols: u16,
    #[schemars(range(min = 1, max = 1000))]
    pub rows: u16,
}

impl Command for TermResize {
    const NAME: &'static str = "term.resize";
    type Args = TermResizeArgs;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new("Resize a terminal (columns × rows).", Toolset::Ai)
    }

    async fn run(ctx: Ctx, args: TermResizeArgs) -> Result<Empty, CmdError> {
        AiService::from_ctx(&ctx)?
            .terminals
            .resize(&args.id, args.cols, args.rows)
            .map_err(not_found)?;
        Ok(Empty {})
    }
}

/// `term.close`
pub struct TermClose;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TermIdArgs {
    pub id: String,
}

impl Command for TermClose {
    const NAME: &'static str = "term.close";
    type Args = TermIdArgs;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new("Close a terminal (kills its shell).", Toolset::Ai).act()
    }

    async fn run(ctx: Ctx, args: TermIdArgs) -> Result<Empty, CmdError> {
        AiService::from_ctx(&ctx)?
            .terminals
            .close(&args.id)
            .map_err(not_found)?;
        Ok(Empty {})
    }
}

/// `term.list`
pub struct TermList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct TermListOutput {
    pub terminals: Vec<TermInfo>,
}

impl Command for TermList {
    const NAME: &'static str = "term.list";
    type Args = Empty;
    type Output = TermListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List open terminals (id, shell, size, pid, exit code).",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<TermListOutput, CmdError> {
        Ok(TermListOutput {
            terminals: AiService::from_ctx(&ctx)?.terminals.list(),
        })
    }
}
