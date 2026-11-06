//! CLI agents: Claude Code and Codex run as child processes and reach the browser through its
//! own MCP endpoint (`http://127.0.0.1:<port>/mcp?agent=<session id>`); we only relay the
//! user's messages and turn their JSON-lines output into `ai.*` events.

pub mod claude;
pub mod codex;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};

/// `claude`: explicit path, else PATH, else `~/.local/bin/claude(.exe)` (the native installer).
pub fn find_claude(explicit: Option<&str>) -> Option<PathBuf> {
    find_binary(explicit, "claude", || {
        let home = home_dir()?;
        let exe = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        Some(vec![home.join(".local").join("bin").join(exe)])
    })
}

/// `codex`: explicit path, else PATH, else the copy bundled with the Codex desktop app
/// (`%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`, newest first).
pub fn find_codex(explicit: Option<&str>) -> Option<PathBuf> {
    find_binary(explicit, "codex", || {
        let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("OpenAI")
            .join("Codex")
            .join("bin");
        let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(base)
            .ok()?
            .flatten()
            .map(|dir| dir.path().join("codex.exe"))
            .filter_map(|exe| Some((exe.metadata().ok()?.modified().ok()?, exe)))
            .collect();
        found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        Some(found.into_iter().map(|(_, p)| p).collect())
    })
}

fn find_binary(
    explicit: Option<&str>,
    name: &str,
    fallbacks: impl FnOnce() -> Option<Vec<PathBuf>>,
) -> Option<PathBuf> {
    if let Some(path) = explicit.filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    which::which(name)
        .ok()
        .or_else(|| fallbacks()?.into_iter().find(|p| p.is_file()))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// A child process command with stdio piped, no console window, killed when dropped.
pub fn command(binary: &Path, cwd: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        // We may be started from inside a Claude Code session (development); the child must
        // not think it is nested.
        .env_remove("CLAUDECODE");
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The last lines a child wrote to stderr (for error messages).
#[derive(Clone, Default)]
pub struct StderrTail(Arc<Mutex<VecDeque<String>>>);

impl StderrTail {
    const KEEP: usize = 20;

    /// Read `stderr` in the background, logging each line.
    pub fn collect(stderr: tokio::process::ChildStderr, who: &'static str) -> Self {
        let tail = StderrTail::default();
        let lines = tail.0.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target: "bmcp_ai::cli", "{who} stderr: {line}");
                let mut lines = lines.lock().unwrap_or_else(|e| e.into_inner());
                lines.push_back(line);
                if lines.len() > Self::KEEP {
                    lines.pop_front();
                }
            }
        });
        tail
    }

    pub fn text(&self) -> String {
        let lines = self.0.lock().unwrap_or_else(|e| e.into_inner());
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}
