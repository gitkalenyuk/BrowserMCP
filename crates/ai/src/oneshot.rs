//! One-shot text completions for background AI features (self-healing in `crates/heal`,
//! `page.extract`): instructions + prompt in, text out. No tools, no session, no events.
//!
//! * API providers: one model turn through [`crate::model::Client`] with no tools;
//! * Claude Code: `claude -p --output-format json --tools "" …`, prompt on stdin;
//! * Codex: `codex exec --json --sandbox read-only -`, prompt on stdin (untested against a real
//!   Codex, like the exec fallback of the agent).
//!
//! The provider is the caller's choice, else [`AiService::default_provider`]: `default_provider`
//! from `ai.json` (`ai.provider_set {id, default: true}`), else the first configured provider in
//! the order of `ai.providers`.

use std::time::Duration;

use bmcp_core::{CmdError, ErrorCode};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::AiService;
use crate::cli::{command, kill_tree, spawn};
use crate::config::{Kind, PRESETS, Resolved};
use crate::model::{Client, Message, Request};
use crate::session::AgentEvent;

/// Default time limit of one completion.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Short fixed system prompt for the CLIs (the real instructions travel in the prompt, because
/// long multi-line arguments cannot be passed safely to `.cmd` shims on Windows).
const CLI_SYSTEM: &str = "You are a precise assistant inside the BrowserMCP browser. Do not use \
     tools. Answer exactly in the format the user message asks for.";

/// What one completion asks for.
#[derive(Debug, Clone, Default)]
pub struct TextRequest {
    /// Provider id from `ai.providers`; `None` = the default provider.
    pub provider: Option<String>,
    /// Model; `None` = the provider's configured model.
    pub model: Option<String>,
    /// Instructions (system prompt for API providers, prepended to the prompt for CLIs).
    pub system: String,
    pub prompt: String,
    /// `None` = [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
}

/// The answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub provider: String,
    pub model: Option<String>,
    pub text: String,
}

impl AiService {
    /// The provider background features use when none is named: `default_provider` from
    /// `ai.json` if it is a known provider, else the first configured one.
    pub fn default_provider(&self) -> Option<String> {
        if let Some(id) = self.config().default_provider
            && crate::config::preset(&id).is_some()
        {
            return Some(id);
        }
        PRESETS
            .iter()
            .find(|p| self.resolve(p.id).is_ok_and(|r| r.configured()))
            .map(|p| p.id.to_string())
    }

    /// Run one completion (see the [module docs](self)).
    pub async fn complete_text(&self, req: TextRequest) -> Result<Completion, CmdError> {
        let provider = match req.provider.clone().filter(|p| !p.is_empty()) {
            Some(provider) => provider,
            None => self.default_provider().ok_or_else(|| {
                CmdError::invalid_params("no AI provider is configured").with_hint(
                    "Configure one with ai_provider_set (an API key, or install Claude Code) and \
                     mark it with `default: true`.",
                )
            })?,
        };
        let resolved = self.resolve(&provider)?;
        if !resolved.configured() {
            return Err(CmdError::invalid_params(format!(
                "AI provider '{provider}' is not configured"
            ))
            .with_hint("Set it up with ai_provider_set, or pass another `provider`."));
        }
        let model = req
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .or(resolved.model.clone());
        let timeout = req.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let work = async {
            match resolved.preset.kind {
                Kind::Api(_) => api_completion(&resolved, model.as_deref(), &req).await,
                Kind::ClaudeCode => {
                    self.claude_completion(&resolved, model.as_deref(), &req)
                        .await
                }
                Kind::Codex => {
                    self.codex_completion(&resolved, model.as_deref(), &req)
                        .await
                }
            }
        };
        let text = tokio::time::timeout(timeout, work).await.map_err(|_| {
            CmdError::new(
                ErrorCode::Timeout,
                format!("AI provider '{provider}' did not answer within {timeout:?}"),
            )
        })??;
        Ok(Completion {
            provider,
            model,
            text,
        })
    }

    /// `claude -p --output-format json`: one JSON object `{type: "result", result, is_error}`.
    async fn claude_completion(
        &self,
        resolved: &Resolved,
        model: Option<&str>,
        req: &TextRequest,
    ) -> Result<String, CmdError> {
        let mut args: Vec<String> = [
            "-p",
            "--output-format",
            "json",
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--system-prompt",
            CLI_SYSTEM,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(model) = model {
            args.extend(["--model".into(), model.into()]);
        }
        let stdout = self
            .run_cli(resolved, &args, &cli_prompt(req), "claude")
            .await?;
        parse_claude_output(&stdout)
    }

    /// `codex exec --json -`: the last agent message is the answer.
    async fn codex_completion(
        &self,
        resolved: &Resolved,
        model: Option<&str>,
        req: &TextRequest,
    ) -> Result<String, CmdError> {
        let mut args: Vec<String> = [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(model) = model {
            args.extend(["-m".into(), model.into()]);
        }
        args.push("-".into());
        let stdout = self
            .run_cli(resolved, &args, &cli_prompt(req), "codex")
            .await?;
        let mut thread = None;
        let mut answer = None;
        for line in stdout.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            for event in crate::cli::codex::exec_event(&v, &mut thread) {
                if let AgentEvent::Text(text) = event {
                    answer = Some(text);
                }
            }
        }
        answer.ok_or_else(|| CmdError::internal("codex exec gave no answer"))
    }

    /// Start a CLI in `data/ai/workspace`, write `prompt` to its stdin, return its stdout.
    async fn run_cli(
        &self,
        resolved: &Resolved,
        args: &[String],
        prompt: &str,
        who: &'static str,
    ) -> Result<String, CmdError> {
        let binary = resolved
            .binary
            .clone()
            .ok_or_else(|| CmdError::invalid_params(format!("{who} is not installed")))?;
        let cwd = self.workspace_dir();
        std::fs::create_dir_all(&cwd)
            .map_err(|e| CmdError::internal(format!("cannot create {}: {e}", cwd.display())))?;
        let mut cmd = command(&binary, &cwd);
        cmd.args(args);
        let (mut child, tree) =
            spawn(&mut cmd).map_err(|e| CmdError::internal(format!("cannot start {who}: {e}")))?;
        let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            kill_tree(&mut child, &tree).await;
            return Err(CmdError::internal(format!("{who}: no stdio pipes")));
        };
        // Write, read stdout and read stderr at the same time so no pipe can fill up.
        let prompt = prompt.to_string();
        let writer = tokio::spawn(async move {
            let _ = stdin.write_all(prompt.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
        let err_reader = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        });
        let mut out = String::new();
        let read = stdout.read_to_string(&mut out).await;
        let status = child.wait().await;
        tree.kill();
        writer.abort();
        let err_text = err_reader.await.unwrap_or_default();
        read.map_err(|e| CmdError::internal(format!("{who}: cannot read output: {e}")))?;
        match status {
            Ok(s) if s.success() => Ok(out),
            Ok(s) => Err(CmdError::internal(format!(
                "{who} exited with {s}: {}",
                crate::model::truncate(
                    if err_text.trim().is_empty() {
                        out.trim()
                    } else {
                        err_text.trim()
                    },
                    800
                )
            ))),
            Err(e) => Err(CmdError::internal(format!("{who}: {e}"))),
        }
    }
}

/// One turn of an HTTP model API without tools.
async fn api_completion(
    resolved: &Resolved,
    model: Option<&str>,
    req: &TextRequest,
) -> Result<String, CmdError> {
    let Kind::Api(api) = resolved.preset.kind else {
        return Err(CmdError::internal("not an API provider"));
    };
    let model = model.ok_or_else(|| {
        CmdError::invalid_params(format!("no model set for '{}'", resolved.preset.id))
            .with_hint("Pass `model`, or set one with ai_provider_set.")
    })?;
    let client = Client::new(api, resolved.base_url.clone(), resolved.api_key.clone());
    let messages = [Message::User {
        text: req.prompt.clone(),
    }];
    let request = Request {
        model,
        system: &req.system,
        messages: &messages,
        tools: &[],
    };
    let reply = client
        .complete(request, &mut |_: &str| {})
        .await
        .map_err(|e| CmdError::internal(format!("AI provider '{}': {e}", resolved.preset.id)))?;
    Ok(reply.text)
}

/// The prompt for a CLI: the instructions first, then the request.
fn cli_prompt(req: &TextRequest) -> String {
    if req.system.is_empty() {
        req.prompt.clone()
    } else {
        format!("{}\n\n---\n\n{}", req.system, req.prompt)
    }
}

/// The `result` text of `claude -p --output-format json` (the last JSON line with a `result`).
pub fn parse_claude_output(stdout: &str) -> Result<String, CmdError> {
    let value = stdout
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.get("result").is_some())
        .or_else(|| serde_json::from_str::<Value>(stdout.trim()).ok())
        .ok_or_else(|| {
            CmdError::internal(format!(
                "unexpected claude output: {}",
                crate::model::truncate(stdout.trim(), 500)
            ))
        })?;
    let text = value["result"].as_str().unwrap_or_default().to_string();
    if value["is_error"] == true {
        return Err(CmdError::internal(format!("claude: {text}")));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_output() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"result":"hi"}"#;
        assert_eq!(parse_claude_output(out).unwrap(), "hi");
        let err = r#"{"type":"result","is_error":true,"result":"limit"}"#;
        assert!(parse_claude_output(err).is_err());
        assert!(parse_claude_output("garbage").is_err());
    }

    #[test]
    fn default_provider_follows_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let ai = AiService::new(dir.path(), "http://127.0.0.1:9/mcp");
        ai.update_config(|c| {
            c.providers.entry("custom".into()).or_default().base_url =
                Some("http://127.0.0.1:9/v1".into());
            c.default_provider = Some("custom".into());
        })
        .unwrap();
        assert_eq!(ai.default_provider().as_deref(), Some("custom"));
        // An unknown id falls back to the first configured provider.
        ai.update_config(|c| c.default_provider = Some("nope".into()))
            .unwrap();
        assert_ne!(ai.default_provider().as_deref(), Some("nope"));
    }
}
