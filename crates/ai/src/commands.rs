//! `ai.*` and `term.*` commands (toolset `ai`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, Registry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Kind, NewProvider, Problem, ProviderChanges, Resolved};
use crate::model::Api;
use crate::session::SessionInfo;
use crate::term::{OpenOptions, TermInfo};
use crate::{AiService, StartOptions, parse_toolsets};

/// Register every command of this crate. The [`AiService`] itself is inserted by the app
/// (it needs the data folder and the MCP URL).
pub fn register(registry: &mut Registry) {
    registry
        .add::<Providers>()
        .add::<ProviderAdd>()
        .add::<ProviderRemove>()
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
    /// e.g. `anthropic`, `openai`, `ollama`, `claude-code`, or a user provider's id
    /// (`openai-work`, `home-server`).
    pub id: String,
    pub name: String,
    /// `api` (BrowserMCP runs the agent loop) or `cli` (Claude Code / Codex).
    pub kind: String,
    /// Wire protocol: `anthropic`, `openai`, `gemini`, `claude-code`, `codex`.
    pub protocol: String,
    /// `true` for the built-in presets; `false` for providers added with ai.provider_add
    /// (those can be renamed and removed).
    pub builtin: bool,
    /// User providers: the preset they were made from (`openai`, `ollama`…), kept when the API
    /// type is changed later; null = blank (and for built-ins).
    pub template: Option<String>,
    /// Ready to start a session (URL and key set / CLI installed).
    pub configured: bool,
    pub needs_key: bool,
    /// Whether an API key is stored (the key itself is not returned).
    pub has_key: bool,
    pub base_url: String,
    /// Model used when `ai.session_start` gives none.
    pub model: Option<String>,
    /// `model` was chosen by the user (ai.provider_set / ai.provider_add); false = it is the
    /// preset's default or the first known model, and follows them.
    pub model_set: bool,
    /// Known models (live list after `ai.provider_test`).
    pub models: Vec<String>,
    /// API providers: names of the extra HTTP headers sent with every request. Their values are
    /// not returned (a header may hold a credential, like the API key).
    pub header_names: Vec<String>,
    /// CLI providers: the binary that will run.
    pub path: Option<String>,
    /// Used by background AI features (self-healing, page.extract) when they name no provider.
    pub is_default: bool,
}

fn provider_info(r: &Resolved, default: Option<&str>) -> ProviderInfo {
    let (kind, protocol) = match r.kind {
        Kind::Api(Api::Anthropic) => ("api", "anthropic"),
        Kind::Api(Api::OpenAi) => ("api", "openai"),
        Kind::Api(Api::Gemini) => ("api", "gemini"),
        Kind::ClaudeCode => ("cli", "claude-code"),
        Kind::Codex => ("cli", "codex"),
    };
    ProviderInfo {
        id: r.id.clone(),
        name: r.name.clone(),
        kind: kind.into(),
        protocol: protocol.into(),
        builtin: r.builtin,
        template: r.template.clone(),
        configured: r.configured(),
        needs_key: r.needs_key,
        has_key: r.api_key.is_some(),
        base_url: r.base_url.clone(),
        model: r.model.clone(),
        model_set: r.settings.model.as_deref().is_some_and(|m| !m.is_empty()),
        models: r.models.clone(),
        header_names: r.headers.keys().cloned().collect(),
        path: r.binary.as_ref().map(|p| p.to_string_lossy().into_owned()),
        is_default: default == Some(r.id.as_str()),
    }
}

/// The provider `id` as `ai.providers` lists it (after a change).
fn info_of(ai: &AiService, id: &str) -> Result<ProviderInfo, CmdError> {
    let default = ai.default_provider();
    Ok(provider_info(&ai.resolve(id)?, default.as_deref()))
}

/// `ai.providers`
pub struct Providers;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProvidersOutput {
    /// Built-in providers first, then the user's own in the order they were added.
    pub providers: Vec<ProviderInfo>,
}

impl Command for Providers {
    const NAME: &'static str = "ai.providers";
    type Args = Empty;
    type Output = ProvidersOutput;

    fn meta() -> Meta {
        Meta::new(
            "List AI providers: the built-ins (Anthropic, OpenAI, Gemini, OpenRouter, Ollama, \
             LM Studio, Claude Code, Codex…) and the user's own (ai.provider_add), with \
             configured/default flags, base URL, default model and known models.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ProvidersOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let default = ai.default_provider();
        let config = ai.config();
        let providers = config
            .ids()
            .iter()
            .map(|id| {
                config
                    .resolve(id)
                    .map(|r| provider_info(&r, default.as_deref()))
            })
            .collect::<Result<_, _>>()?;
        Ok(ProvidersOutput { providers })
    }
}

/// `ai.provider_add`
pub struct ProviderAdd;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderAddArgs {
    /// Display name, unique among all providers, e.g. "OpenAI (work)", "Home server".
    pub name: String,
    /// Preset to start from: an API provider id from ai.providers ("openai", "openrouter",
    /// "ollama", "lmstudio", "groq", "anthropic", "gemini"…) or "custom" for a blank
    /// endpoint (default). Gives the API type, base URL, suggested models and whether a key is
    /// required.
    #[serde(default)]
    pub template: Option<String>,
    /// Id for ai.session_start and the other ai.* commands; default = made from the name
    /// ("home-server"). Lowercase letters, digits, '-', '_', '.'.
    #[serde(default)]
    pub id: Option<String>,
    /// Wire protocol; default = the template's ("openai" for a blank one).
    #[serde(default)]
    pub api: Option<Api>,
    /// API root, e.g. "http://10.0.0.5:8000/v1"; default = the template's (required for a blank
    /// one).
    #[serde(default)]
    pub base_url: Option<String>,
    /// API key. Stored in plain JSON in data/config/ai.json.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Default model (ai.provider_test lists the endpoint's models).
    #[serde(default)]
    pub model: Option<String>,
    /// Extra HTTP headers sent with every request, e.g. {"HTTP-Referer": "https://me.test"}.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl Command for ProviderAdd {
    const NAME: &'static str = "ai.provider_add";
    type Args = ProviderAddArgs;
    type Output = ProviderInfo;

    fn meta() -> Meta {
        Meta::new(
            "Add your own AI provider (any number: two OpenAI accounts, several local servers…), \
             starting from a template preset or a blank OpenAI-compatible endpoint. Saved in \
             data/config/ai.json. Returns the provider; its id works everywhere a built-in id does.",
            Toolset::Ai,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: ProviderAddArgs) -> Result<ProviderInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let new = NewProvider {
            name: args.name,
            template: args.template,
            id: args.id,
            api: args.api,
            base_url: args.base_url,
            api_key: args.api_key,
            model: args.model,
            headers: args.headers,
        };
        let id = ai.try_update_config(|config| config.add_provider(new))?;
        info_of(&ai, &id)
    }
}

/// `ai.provider_remove`
pub struct ProviderRemove;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProviderRemoveOutput {
    pub id: String,
    /// It was the default of background AI features; the default is automatic again.
    pub was_default: bool,
}

impl Command for ProviderRemove {
    const NAME: &'static str = "ai.provider_remove";
    type Args = ProviderIdArgs;
    type Output = ProviderRemoveOutput;

    fn meta() -> Meta {
        Meta::new(
            "Remove a provider added with ai.provider_add (its key and settings are deleted from \
             data/config/ai.json). Built-in providers cannot be removed.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ProviderIdArgs) -> Result<ProviderRemoveOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let was_default = ai.try_update_config(|config| config.remove_provider(&args.id))?;
        Ok(ProviderRemoveOutput {
            id: args.id,
            was_default,
        })
    }
}

/// `ai.provider_set`
pub struct ProviderSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSetArgs {
    /// Provider id from ai.providers (built-in or added with ai.provider_add).
    pub id: String,
    /// User providers: new display name (unique among all providers).
    #[serde(default)]
    pub name: Option<String>,
    /// User providers: wire protocol ("openai" = OpenAI-compatible, "anthropic", "gemini").
    #[serde(default)]
    pub api: Option<Api>,
    /// API key; "" removes it. Stored in plain JSON in data/config/ai.json.
    #[serde(default)]
    pub api_key: Option<String>,
    /// API root, e.g. "http://localhost:11434/v1"; "" = preset default (built-ins) or none.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Default model; "" = preset default.
    #[serde(default)]
    pub model: Option<String>,
    /// CLI providers: path of the claude / codex binary; "" = search PATH.
    #[serde(default)]
    pub path: Option<String>,
    /// API providers: extra HTTP headers; replaces all of them ({} removes them). A null value
    /// keeps that header's stored value (ai.providers lists header names only), e.g.
    /// {"api-key": null, "X-Title": "Mine"}.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, Option<String>>>,
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
            "Change any provider: API key, base URL, default model, extra headers, CLI path, and \
             for your own providers the name and API type (saved to data/config/ai.json). \
             Omitted fields stay unchanged; \"\" clears one. Returns the provider.",
            Toolset::Ai,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: ProviderSetArgs) -> Result<ProviderInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let changes = ProviderChanges {
            name: args.name,
            api: args.api,
            api_key: args.api_key,
            base_url: args.base_url,
            model: args.model,
            path: args.path,
            headers: args.headers,
            default: args.default,
        };
        ai.try_update_config(|config| config.update_provider(&args.id, changes))?;
        info_of(&ai, &args.id)
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
    /// Set when the provider is not set up (nothing was sent): `no_base_url`, `no_api_key` or
    /// `not_installed`. `error` then says the same in English.
    pub problem: Option<Problem>,
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
            problem: None,
        };
        // Say what is missing instead of sending a request that must fail.
        if let Some(problem) = r.problem() {
            out.error = Some(match problem {
                Problem::NoBaseUrl => "no base URL set".into(),
                Problem::NoApiKey => "no API key set".into(),
                Problem::NotInstalled => format!("{} not found", r.name),
            });
            out.problem = Some(problem);
            return Ok(out);
        }
        match r.kind {
            Kind::Api(_) => {
                let client = r.client().expect("an API provider");
                match client.list_models().await {
                    Ok(models) => {
                        out.ok = true;
                        if !models.is_empty() {
                            let saved = models.clone();
                            ai.update_config(|c| {
                                // The provider may have been removed meanwhile.
                                if let Some(settings) = c.settings_mut(&args.id) {
                                    settings.models = saved;
                                }
                            })?;
                        }
                        out.models = models;
                    }
                    Err(err) => out.error = Some(err.to_string()),
                }
            }
            Kind::ClaudeCode | Kind::Codex => {
                let binary = r
                    .binary
                    .as_ref()
                    .expect("problem() checked it is installed");
                match cli_version(binary).await {
                    Ok(version) => {
                        out.ok = true;
                        out.version = Some(version);
                    }
                    Err(err) => out.error = Some(err),
                }
            }
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
