//! `ai.*` and `term.*` commands (toolset `ai`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, Registry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::access::{AccessSettings, CodexApproval, CodexSandbox, PermissionMode};
use crate::cli::permission::{Decision, PermissionRequest};
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
        .add::<SessionClose>()
        .add::<SessionReopen>()
        .add::<SessionDelete>()
        .add::<SessionRename>()
        .add::<SessionSetModel>()
        .add::<SessionInterrupt>()
        .add::<SessionEdit>()
        .add::<Models>()
        .add::<SessionsList>()
        .add::<SessionTranscript>()
        .add::<PermissionsList>()
        .add::<PermissionAnswer>()
        .add::<AgentAccessGet>()
        .add::<AgentAccessSet>()
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
    /// Claude Code: who approves tool calls outside the browser's tools: "ask" (an approval
    /// card per call), "accept_edits", "full_access", "plan", "browser_only". Default: the
    /// provider's setting (ai.agent_access).
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// CLI agents: working folder (must exist). Default: the provider's setting, else
    /// data/ai/workspace.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Claude Code: more folders the agent may use (must exist). Default: the provider's;
    /// [] = none.
    #[serde(default)]
    pub add_dirs: Option<Vec<String>>,
    /// Codex: shell sandbox "read_only", "workspace_write" or "full_access".
    #[serde(default)]
    pub sandbox: Option<CodexSandbox>,
    /// Codex: "untrusted" (ask for most commands), "on_request" (the agent asks when it
    /// needs to leave the sandbox) or "never".
    #[serde(default)]
    pub approval_policy: Option<CodexApproval>,
}

impl Command for SessionStart {
    const NAME: &'static str = "ai.session_start";
    type Args = SessionStartArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Start an AI agent session that drives the browser through BrowserMCP's tools. Progress \
             arrives as ai.message / ai.tool_call / ai.tool_result / ai.done events; CLI agents \
             may ask for approval (ai.permission events, answer with ai.permission_answer). \
             The browser's own agents may not start a chat with more access than the \
             provider's defaults. Returns the session (id = agent id, color, status).",
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
            access: AccessSettings {
                permission_mode: args.permission_mode,
                cwd: args.cwd,
                add_dirs: args.add_dirs,
                sandbox: args.sandbox,
                approval_policy: args.approval_policy,
            },
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
            "Send the next user message to an AI session. Never refused while it works: the \
             agent takes it during the current turn (Claude Code folds it in, Codex steers, \
             API agents add it before the next model call) or right after; until then the \
             ai.message event carries queued:true and SessionInfo.queued counts it \
             (ai.message_taken follows). A stopped or failed session continues the same \
             conversation. Returns the session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionSendArgs) -> Result<SessionInfo, CmdError> {
        if args.text.trim().is_empty() {
            return Err(CmdError::invalid_params("text is empty"));
        }
        AiService::from_ctx(&ctx)?
            .send(&ctx, &args.id, args.text)
            .await
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
            "Stop an AI session: cancels the running model call / kills the CLI agent and its \
             pending approval requests. The session stays (ai.session_send continues it). \
             Returns the session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionIdArgs) -> Result<SessionInfo, CmdError> {
        AiService::from_ctx(&ctx)?.stop(&args.id).await
    }
}

/// `ai.session_close`
pub struct SessionClose;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCloseArgs {
    /// Session id.
    pub id: String,
    /// Older form of ai.session_delete: delete the chat and all its files instead of
    /// archiving it. Default false.
    #[serde(default)]
    pub delete_transcript: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionCloseOutput {
    /// The closed session.
    pub closed: String,
}

impl Command for SessionClose {
    const NAME: &'static str = "ai.session_close";
    type Args = SessionCloseArgs;
    type Output = SessionCloseOutput;

    fn meta() -> Meta {
        Meta::new(
            "Close an AI chat: stops it if running and archives it — it leaves the open chats \
             (ai.sessions_list) but keeps every file and is listed with \
             ai.sessions_list {archived: true}; ai.session_reopen brings it back. Emits \
             ai.session_closed. delete_transcript: true deletes it instead (ai.session_delete).",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionCloseArgs) -> Result<SessionCloseOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        if args.delete_transcript {
            ai.refuse_other_agents_chat(&ctx.caller, &args.id, "delete")?;
        }
        ai.close(&ctx, &args.id, args.delete_transcript).await?;
        Ok(SessionCloseOutput { closed: args.id })
    }
}

/// `ai.session_reopen`
pub struct SessionReopen;

impl Command for SessionReopen {
    const NAME: &'static str = "ai.session_reopen";
    type Args = SessionIdArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Reopen an archived (closed) AI chat: it is among the open chats again with its \
             history; the next ai.session_send continues the conversation like a stopped chat.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionIdArgs) -> Result<SessionInfo, CmdError> {
        AiService::from_ctx(&ctx)?.reopen(&ctx, &args.id).await
    }
}

/// `ai.session_delete`
pub struct SessionDelete;

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionDeleteOutput {
    pub deleted: bool,
}

impl Command for SessionDelete {
    const NAME: &'static str = "ai.session_delete";
    type Args = SessionIdArgs;
    type Output = SessionDeleteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete an AI chat for good: stops it if running and removes all its files (meta, \
             transcript, history). Emits ai.session_closed and ai.session_deleted. An AI agent \
             may delete only its own chat (NEEDS_APPROVAL otherwise).",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionIdArgs) -> Result<SessionDeleteOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        ai.refuse_other_agents_chat(&ctx.caller, &args.id, "delete")?;
        ai.delete(&ctx, &args.id).await?;
        Ok(SessionDeleteOutput { deleted: true })
    }
}

/// `ai.session_rename`
pub struct SessionRename;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRenameArgs {
    /// Session id.
    pub id: String,
    /// New title; "" = back to the automatic title (the first message).
    pub title: String,
}

impl Command for SessionRename {
    const NAME: &'static str = "ai.session_rename";
    type Args = SessionRenameArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Rename an AI chat (saved); \"\" goes back to the automatic title. An AI agent may \
             rename only its own chat (NEEDS_APPROVAL otherwise). Returns the session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionRenameArgs) -> Result<SessionInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        ai.refuse_other_agents_chat(&ctx.caller, &args.id, "rename")?;
        ai.rename(&ctx, &args.id, &args.title)
    }
}

/// `ai.session_set_model`
pub struct SessionSetModel;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSetModelArgs {
    /// Session id.
    pub id: String,
    /// A model id from ai.models; "" or "default" = the provider's default.
    pub model: String,
}

impl Command for SessionSetModel {
    const NAME: &'static str = "ai.session_set_model";
    type Args = SessionSetModelArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Change the model of an AI chat from its next turn on (saved). A running Claude \
             Code switches at once (set_model), Codex uses it on the next turn/start, API \
             agents on the next request. Returns the session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionSetModelArgs) -> Result<SessionInfo, CmdError> {
        AiService::from_ctx(&ctx)?.set_model(&ctx, &args.id, &args.model)
    }
}

/// `ai.session_interrupt`
pub struct SessionInterrupt;

impl Command for SessionInterrupt {
    const NAME: &'static str = "ai.session_interrupt";
    type Args = SessionIdArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Interrupt the current turn of an AI chat but keep the chat (and the CLI agent) \
             alive: the answer ends with ai.done {reason: stopped} and the next message goes \
             on in the same conversation. ai.session_stop also ends the agent process.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionIdArgs) -> Result<SessionInfo, CmdError> {
        AiService::from_ctx(&ctx)?.interrupt(&ctx, &args.id).await
    }
}

/// `ai.session_edit`
pub struct SessionEdit;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionEditArgs {
    /// Session id.
    pub id: String,
    /// 0-based number of the USER message in the chat (`index` of ai.message / transcript
    /// entries).
    pub index: usize,
    /// The new text of that message.
    pub text: String,
}

impl Command for SessionEdit {
    const NAME: &'static str = "ai.session_edit";
    type Args = SessionEditArgs;
    type Output = SessionInfo;

    fn meta() -> Meta {
        Meta::new(
            "Edit an earlier user message of an AI chat: the conversation is rewound to just \
             before it (the transcript is cut there, ai.session_rewound) and the new text is \
             sent instead, as the next user message. Stops the current turn first. Returns the \
             session.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SessionEditArgs) -> Result<SessionInfo, CmdError> {
        if args.text.trim().is_empty() {
            return Err(CmdError::invalid_params("text is empty"));
        }
        AiService::from_ctx(&ctx)?
            .edit(&ctx, &args.id, args.index, args.text)
            .await
    }
}

/// `ai.models`
pub struct Models;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelsArgs {
    /// Provider id from ai.providers.
    pub provider: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ModelsOutput {
    pub models: Vec<crate::models::ModelChoice>,
    /// The model new chats of this provider use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl Command for Models {
    const NAME: &'static str = "ai.models";
    type Args = ModelsArgs;
    type Output = ModelsOutput;

    fn meta() -> Meta {
        Meta::new(
            "The models a provider offers, for ai.session_start / ai.session_set_model: Claude \
             Code's aliases and model ids, Codex's model/list, an API provider's live list \
             (no tokens spent) or its configured models.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, args: ModelsArgs) -> Result<ModelsOutput, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let resolved = ai.resolve(&args.provider)?;
        let (models, default) = crate::models::list(&resolved, &ai.default_cwd()).await;
        Ok(ModelsOutput { models, default })
    }
}

/// `ai.sessions_list`
pub struct SessionsList;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionsListArgs {
    /// true: the archived (closed) chats, most recently active first; default: the open
    /// chats.
    #[serde(default)]
    pub archived: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionsListOutput {
    pub sessions: Vec<SessionInfo>,
}

impl Command for SessionsList {
    const NAME: &'static str = "ai.sessions_list";
    type Args = SessionsListArgs;
    type Output = SessionsListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List AI chats (agents) with id, title, provider, model, color, status, the tabs \
             each touched, access settings, pending approval requests and queued messages. \
             Default: the open chats; archived: true lists the closed ones, newest first.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, args: SessionsListArgs) -> Result<SessionsListOutput, CmdError> {
        Ok(SessionsListOutput {
            sessions: AiService::from_ctx(&ctx)?.sessions_filtered(args.archived.unwrap_or(false)),
        })
    }
}

/// `ai.session_transcript`
pub struct SessionTranscript;

#[derive(Debug, Serialize, JsonSchema)]
pub struct TranscriptOutput {
    pub id: String,
    /// Entries `{type: user|assistant|tool_call|tool_result|notice|done|permission, ts_ms, …}`.
    pub entries: Vec<Value>,
}

impl Command for SessionTranscript {
    const NAME: &'static str = "ai.session_transcript";
    type Args = SessionIdArgs;
    type Output = TranscriptOutput;

    fn meta() -> Meta {
        Meta::new(
            "The transcript of an AI session: user and assistant messages, tool calls and \
             results, answered approval requests.",
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

// ---------------------------------------------------------------- approvals

/// `ai.permissions_list`
pub struct PermissionsList;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionsListArgs {
    /// Only this session's requests; default all sessions.
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PermissionsListOutput {
    /// Oldest first.
    pub permissions: Vec<PermissionRequest>,
}

impl Command for PermissionsList {
    const NAME: &'static str = "ai.permissions_list";
    type Args = PermissionsListArgs;
    type Output = PermissionsListOutput;

    fn meta() -> Meta {
        Meta::new(
            "Approval requests of CLI agents waiting for an answer (tool, input, reason). New ones \
             arrive as ai.permission {status: pending} events.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, args: PermissionsListArgs) -> Result<PermissionsListOutput, CmdError> {
        Ok(PermissionsListOutput {
            permissions: AiService::from_ctx(&ctx)?.permissions(args.session.as_deref())?,
        })
    }
}

/// `ai.permission_answer`
pub struct PermissionAnswer;

/// The answer to an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChoice {
    /// Run it this once.
    Allow,
    /// Run it, and allow this tool for the rest of the chat.
    AllowAlways,
    /// Do not run it (give a reason to tell the agent why).
    Deny,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionAnswerArgs {
    /// Request id from ai.permissions_list / the ai.permission event.
    pub id: String,
    pub decision: PermissionChoice,
    /// Deny: why, passed on to the agent.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PermissionAnswerOutput {
    /// The answered request, plus others resolved with it (allow_always answers every waiting
    /// request for the same tool).
    pub resolved: Vec<PermissionRequest>,
}

impl Command for PermissionAnswer {
    const NAME: &'static str = "ai.permission_answer";
    type Args = PermissionAnswerArgs;
    type Output = PermissionAnswerOutput;

    fn meta() -> Meta {
        Meta::new(
            "Answer a CLI agent's approval request: allow, allow_always (this tool, for the rest \
             of the chat) or deny with an optional reason. For people and the clients they \
             connected: the browser's own AI agents get NEEDS_APPROVAL.",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PermissionAnswerArgs) -> Result<PermissionAnswerOutput, CmdError> {
        let decision = match args.decision {
            PermissionChoice::Allow => Decision::Allow,
            PermissionChoice::AllowAlways => Decision::AllowAlways,
            PermissionChoice::Deny => Decision::Deny {
                reason: args.reason.filter(|r| !r.trim().is_empty()),
            },
        };
        let resolved =
            AiService::from_ctx(&ctx)?.answer_permission(&ctx.caller, &args.id, decision)?;
        Ok(PermissionAnswerOutput { resolved })
    }
}

// ---------------------------------------------------------------- agent access

/// What CLI agents of a provider may do by default, as `ai.agent_access` returns it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AgentAccessInfo {
    pub provider: String,
    /// Claude Code permission mode of new chats.
    pub permission_mode: PermissionMode,
    /// Working folder of new chats; null = `default_cwd`.
    pub cwd: Option<String>,
    /// The folder used when none is set (data/ai/workspace).
    pub default_cwd: String,
    /// Claude Code: more folders the agent may use.
    pub add_dirs: Vec<String>,
    /// Codex shell sandbox of new chats.
    pub sandbox: CodexSandbox,
    /// Codex approval policy of new chats.
    pub approval_policy: CodexApproval,
}

fn access_info(ai: &AiService, provider: &str, a: AccessSettings) -> AgentAccessInfo {
    AgentAccessInfo {
        provider: provider.to_string(),
        permission_mode: a.permission_mode.unwrap_or_default(),
        cwd: a.cwd,
        default_cwd: ai.default_cwd().to_string_lossy().into_owned(),
        add_dirs: a.add_dirs.unwrap_or_default(),
        sandbox: a.sandbox.unwrap_or_default(),
        approval_policy: a.approval_policy.unwrap_or_default(),
    }
}

/// `ai.agent_access`
pub struct AgentAccessGet;

impl Command for AgentAccessGet {
    const NAME: &'static str = "ai.agent_access";
    type Args = ProviderIdArgs;
    type Output = AgentAccessInfo;

    fn meta() -> Meta {
        Meta::new(
            "Default permissions of a CLI agent provider (claude-code, codex) for new chats: \
             permission mode, working folder, extra folders, Codex sandbox and approval policy.",
            Toolset::Ai,
        )
    }

    async fn run(ctx: Ctx, args: ProviderIdArgs) -> Result<AgentAccessInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        let access = ai.access_settings(&args.id)?;
        Ok(access_info(&ai, &args.id, access))
    }
}

/// `ai.agent_access_set`
pub struct AgentAccessSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentAccessSetArgs {
    /// Provider id (claude-code, codex).
    pub id: String,
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    /// Working folder (must exist); "" = the default (data/ai/workspace).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Replaces the extra folders (each must exist); [] clears them.
    #[serde(default)]
    pub add_dirs: Option<Vec<String>>,
    #[serde(default)]
    pub sandbox: Option<CodexSandbox>,
    #[serde(default)]
    pub approval_policy: Option<CodexApproval>,
}

impl Command for AgentAccessSet {
    const NAME: &'static str = "ai.agent_access_set";
    type Args = AgentAccessSetArgs;
    type Output = AgentAccessInfo;

    fn meta() -> Meta {
        Meta::new(
            "Change the default permissions of a CLI agent provider for new chats (saved to \
             data/config/ai.json). Omitted fields stay unchanged. Running chats keep theirs. \
             Not for the browser's own AI agents (NEEDS_APPROVAL).",
            Toolset::Ai,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: AgentAccessSetArgs) -> Result<AgentAccessInfo, CmdError> {
        let ai = AiService::from_ctx(&ctx)?;
        ai.refuse_own_agent(&ctx.caller, "change what AI agents may do")?;
        let access = ai.set_access_settings(&args.id, |a| {
            if args.permission_mode.is_some() {
                a.permission_mode = args.permission_mode;
            }
            if let Some(cwd) = args.cwd {
                a.cwd = Some(cwd.trim().to_string()).filter(|c| !c.is_empty());
            }
            if let Some(dirs) = args.add_dirs {
                let dirs: Vec<String> = dirs
                    .into_iter()
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty())
                    .collect();
                // The provider's own setting: none is simply unset.
                a.add_dirs = Some(dirs).filter(|d| !d.is_empty());
            }
            if args.sandbox.is_some() {
                a.sandbox = args.sandbox;
            }
            if args.approval_policy.is_some() {
                a.approval_policy = args.approval_policy;
            }
        })?;
        Ok(access_info(&ai, &args.id, access))
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
