//! What a CLI agent (Claude Code, Codex) may do on this computer, and where it works.
//!
//! The browser's own MCP tools are always allowed; everything else (shell, file edits, web
//! fetches…) follows the permission mode. Defaults are per provider, saved in
//! `data/config/ai.json` under `providers.<id>.access`, and every new chat may override them:
//!
//! ```json
//! { "providers": { "claude-code": { "access": {
//!     "permission_mode": "ask", "cwd": "D:\\work", "add_dirs": ["D:\\docs"] } } } }
//! ```

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Claude Code: who decides about tool calls outside the browser's MCP tools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Every tool call that needs permission asks the user (an approval card in the chat).
    #[default]
    Ask,
    /// File edits are accepted without asking; everything else still asks.
    AcceptEdits,
    /// Nothing asks: the agent may run any command and change any file.
    FullAccess,
    /// The agent only reads and plans; it asks before leaving plan mode.
    Plan,
    /// Only the browser's MCP tools; everything else is refused without asking.
    BrowserOnly,
}

impl PermissionMode {
    /// The `--permission-mode` value, or `None` for Claude Code's own default mode.
    pub fn cli_mode(self) -> Option<&'static str> {
        match self {
            PermissionMode::Ask => None,
            PermissionMode::AcceptEdits => Some("acceptEdits"),
            PermissionMode::FullAccess => Some("bypassPermissions"),
            PermissionMode::Plan => Some("plan"),
            PermissionMode::BrowserOnly => Some("dontAsk"),
        }
    }

    /// Whether permission prompts reach us (`--permission-prompt-tool stdio`). In browser-only
    /// mode nothing may prompt: anything that would is refused.
    pub fn prompts(self) -> bool {
        self != PermissionMode::BrowserOnly
    }
}

/// Codex: the shell sandbox of a thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodexSandbox {
    /// Commands may read files but not change them.
    ReadOnly,
    /// Commands may change files in the working folder.
    #[default]
    WorkspaceWrite,
    /// No sandbox at all.
    FullAccess,
}

impl CodexSandbox {
    /// The app-server / `codex exec --sandbox` value.
    pub fn wire(self) -> &'static str {
        match self {
            CodexSandbox::ReadOnly => "read-only",
            CodexSandbox::WorkspaceWrite => "workspace-write",
            CodexSandbox::FullAccess => "danger-full-access",
        }
    }
}

/// Codex: when it asks before running a command or changing files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodexApproval {
    /// Ask for anything that is not a known-safe read.
    Untrusted,
    /// The agent asks when it needs to leave the sandbox.
    #[default]
    OnRequest,
    /// Never ask; whatever the sandbox refuses fails.
    Never,
}

impl CodexApproval {
    /// The app-server `approvalPolicy` value.
    pub fn wire(self) -> &'static str {
        match self {
            CodexApproval::Untrusted => "untrusted",
            CodexApproval::OnRequest => "on-request",
            CodexApproval::Never => "never",
        }
    }
}

/// Saved per provider (`providers.<id>.access`) or passed to `ai.session_start`. Unset fields
/// fall back to the provider's settings, then to the defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccessSettings {
    /// Claude Code permission mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// Working folder; unset = `data/ai/workspace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Claude Code: more folders the agent may use (`--add-dir`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_dirs: Vec<String>,
    /// Codex shell sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<CodexSandbox>,
    /// Codex approval policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<CodexApproval>,
}

impl AccessSettings {
    pub fn is_default(&self) -> bool {
        *self == AccessSettings::default()
    }

    /// `over` wins where it sets something (an empty `add_dirs` keeps ours).
    pub fn overridden_by(&self, over: &AccessSettings) -> AccessSettings {
        AccessSettings {
            permission_mode: over.permission_mode.or(self.permission_mode),
            cwd: over
                .cwd
                .clone()
                .filter(|c| !c.trim().is_empty())
                .or_else(|| self.cwd.clone()),
            add_dirs: if over.add_dirs.is_empty() {
                self.add_dirs.clone()
            } else {
                over.add_dirs.clone()
            },
            sandbox: over.sandbox.or(self.sandbox),
            approval_policy: over.approval_policy.or(self.approval_policy),
        }
    }

    /// Fill in the defaults and check that the folders exist. `default_cwd` is used when no
    /// folder is set (it is created if missing).
    pub fn resolve(&self, default_cwd: &Path) -> Result<Access, String> {
        let cwd = match self.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(dir) => existing_dir(dir)?,
            None => {
                std::fs::create_dir_all(default_cwd)
                    .map_err(|e| format!("cannot create {}: {e}", default_cwd.display()))?;
                default_cwd.to_path_buf()
            }
        };
        let add_dirs = self
            .add_dirs
            .iter()
            .map(|d| d.trim())
            .filter(|d| !d.is_empty())
            .map(existing_dir)
            .collect::<Result<_, _>>()?;
        Ok(Access {
            permission_mode: self.permission_mode.unwrap_or_default(),
            cwd,
            add_dirs,
            sandbox: self.sandbox.unwrap_or_default(),
            approval_policy: self.approval_policy.unwrap_or_default(),
        })
    }
}

fn existing_dir(dir: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(dir);
    if path.is_dir() {
        Ok(path)
    } else {
        Err(format!("folder not found: {dir}"))
    }
}

/// What one session actually uses (also reported by `ai.sessions_list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Access {
    pub permission_mode: PermissionMode,
    /// Working folder of the agent.
    pub cwd: PathBuf,
    pub add_dirs: Vec<PathBuf>,
    pub sandbox: CodexSandbox,
    pub approval_policy: CodexApproval,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_and_defaults() {
        let saved = AccessSettings {
            permission_mode: Some(PermissionMode::AcceptEdits),
            add_dirs: vec!["a".into()],
            sandbox: Some(CodexSandbox::ReadOnly),
            ..Default::default()
        };
        let chat = AccessSettings {
            permission_mode: Some(PermissionMode::Plan),
            cwd: Some("  ".into()), // blank = not set
            ..Default::default()
        };
        let merged = saved.overridden_by(&chat);
        assert_eq!(merged.permission_mode, Some(PermissionMode::Plan));
        assert_eq!(merged.cwd, None);
        assert_eq!(merged.add_dirs, vec!["a".to_string()]);
        assert_eq!(merged.sandbox, Some(CodexSandbox::ReadOnly));
        assert!(AccessSettings::default().is_default());
    }

    #[test]
    fn resolve_checks_folders_and_creates_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("ai").join("workspace");
        let access = AccessSettings::default().resolve(&workspace).unwrap();
        assert!(workspace.is_dir());
        assert_eq!(access.cwd, workspace);
        assert_eq!(access.permission_mode, PermissionMode::Ask);
        assert_eq!(access.sandbox, CodexSandbox::WorkspaceWrite);
        assert_eq!(access.approval_policy, CodexApproval::OnRequest);

        let missing = dir.path().join("nope").to_string_lossy().into_owned();
        let err = AccessSettings {
            cwd: Some(missing.clone()),
            ..Default::default()
        }
        .resolve(&workspace)
        .unwrap_err();
        assert!(err.contains("nope"), "{err}");
        let err = AccessSettings {
            add_dirs: vec![missing],
            ..Default::default()
        }
        .resolve(&workspace)
        .unwrap_err();
        assert!(err.contains("folder not found"), "{err}");

        let own = dir.path().to_string_lossy().into_owned();
        let access = AccessSettings {
            cwd: Some(own.clone()),
            add_dirs: vec![own.clone()],
            ..Default::default()
        }
        .resolve(&workspace)
        .unwrap();
        assert_eq!(access.cwd, dir.path());
        assert_eq!(access.add_dirs, vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn wire_values() {
        assert_eq!(PermissionMode::Ask.cli_mode(), None);
        assert_eq!(
            PermissionMode::FullAccess.cli_mode(),
            Some("bypassPermissions")
        );
        assert_eq!(PermissionMode::BrowserOnly.cli_mode(), Some("dontAsk"));
        assert!(!PermissionMode::BrowserOnly.prompts());
        assert!(PermissionMode::Plan.prompts());
        assert_eq!(CodexSandbox::FullAccess.wire(), "danger-full-access");
        assert_eq!(CodexApproval::OnRequest.wire(), "on-request");
        let json = serde_json::to_value(PermissionMode::AcceptEdits).unwrap();
        assert_eq!(json, "accept_edits");
    }
}
