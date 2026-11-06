//! CLI agents: Claude Code and Codex run as child processes and reach the browser through its
//! own MCP endpoint (`http://127.0.0.1:<port>/mcp?agent=<session id>`); we only relay the
//! user's messages and turn their JSON-lines output into `ai.*` events.

pub mod claude;
pub mod codex;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::task::JoinHandle;

pub use crate::proc_tree::ProcessTree;

/// `claude`: explicit path, else PATH, else `~/.local/bin/claude(.exe)` (the native installer).
/// A native `claude.exe` wins over an npm `claude.cmd` shim found on PATH: the shim runs the
/// agent as a grandchild under `cmd.exe`, and quotes arguments through batch-file rules.
pub fn find_claude(explicit: Option<&str>) -> Option<PathBuf> {
    find_binary(explicit, "claude", true, || {
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
    find_binary(explicit, "codex", false, || {
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

/// `prefer_native`: use an existing fallback instead of a `.cmd` / `.bat` shim found on PATH.
fn find_binary(
    explicit: Option<&str>,
    name: &str,
    prefer_native: bool,
    fallbacks: impl FnOnce() -> Option<Vec<PathBuf>>,
) -> Option<PathBuf> {
    if let Some(path) = explicit.filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    pick_binary(which::which(name).ok(), prefer_native, fallbacks)
}

/// The PATH hit, unless it is missing or (with `prefer_native`) a batch shim and an existing
/// fallback is available.
fn pick_binary(
    on_path: Option<PathBuf>,
    prefer_native: bool,
    fallbacks: impl FnOnce() -> Option<Vec<PathBuf>>,
) -> Option<PathBuf> {
    let is_shim = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    };
    match on_path {
        Some(path) if !(prefer_native && is_shim(&path)) => Some(path),
        on_path => fallbacks()
            .and_then(|list| list.into_iter().find(|p| p.is_file()))
            .or(on_path),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Spawn `cmd` and start tracking its process tree (see [`ProcessTree`]).
pub fn spawn(cmd: &mut tokio::process::Command) -> std::io::Result<(Child, ProcessTree)> {
    let child = cmd.spawn()?;
    let tree = ProcessTree::new(child.id());
    Ok((child, tree))
}

/// How long a stdout reader may keep draining after its process was stopped or exited.
/// Tests use a much longer grace so "finished quickly" can only mean "the process tree was
/// killed and stdout closed", never "the grace period ran out" — robust on a loaded machine.
const READER_GRACE: Duration = if cfg!(test) {
    Duration::from_secs(20)
} else {
    Duration::from_secs(3)
};

/// Wait for a stdout reader task to see EOF; abort it if the pipe stays open (some process we
/// could not kill still holds it), so the session task always finishes.
pub async fn join_reader(mut reader: JoinHandle<()>) {
    if tokio::time::timeout(READER_GRACE, &mut reader)
        .await
        .is_err()
    {
        reader.abort();
    }
}

/// Stop a running CLI child: kill its whole process tree, then reap it.
pub async fn kill_tree(child: &mut Child, tree: &ProcessTree) {
    tree.kill();
    let _ = child.kill().await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_binary_beats_a_batch_shim() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("claude.exe");
        std::fs::write(&native, b"").unwrap();
        let shim = dir.path().join("claude.cmd");
        let missing = dir.path().join("nope.exe");
        let fallbacks = || Some(vec![missing.clone(), native.clone()]);

        // A shim on PATH is replaced by the native install when asked to...
        assert_eq!(
            pick_binary(Some(shim.clone()), true, fallbacks),
            Some(native.clone())
        );
        // ...but kept when not asked, or when there is no native install.
        assert_eq!(
            pick_binary(Some(shim.clone()), false, fallbacks),
            Some(shim.clone())
        );
        assert_eq!(
            pick_binary(Some(shim.clone()), true, || Some(vec![missing.clone()])),
            Some(shim.clone())
        );
        // A real executable on PATH always wins; nothing on PATH uses the fallbacks.
        let on_path = dir.path().join("bin").join("claude.exe");
        assert_eq!(
            pick_binary(Some(on_path.clone()), true, fallbacks),
            Some(on_path)
        );
        assert_eq!(pick_binary(None, false, fallbacks), Some(native));
    }
}
