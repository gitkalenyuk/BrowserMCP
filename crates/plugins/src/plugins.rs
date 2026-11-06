//! Installed plugins and their script host processes.
//!
//! Each enabled plugin gets a supervisor task that runs
//!
//! ```text
//! bmcp-script-host run data/plugins/<name>/<main> --ws ws://127.0.0.1:<port>/v1/ws?plugin=<name>&token=<t> --input plugin=<name>
//! ```
//!
//! without a timeout. The token is fresh random hex for every start; the API accepts the
//! connection only while it is the plugin's current token ([`PluginAuth`]), so disabling a
//! plugin revokes its access even before its process is gone. stdout/stderr lines go into a
//! ring buffer (`plugins.logs`). A process that exits with 0 is done (`exited`: a plugin with
//! no handlers); any other exit restarts it with a growing pause, and after
//! [`MAX_QUICK_FAILURES`] failures in a row it stays `crashed` until enabled or reloaded again.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bmcp_api::access::{PluginAuth, PluginGrant};
use bmcp_core::{CmdError, ErrorCode};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;

use crate::manifest::{MANIFEST_FILE, Manifest, PaletteEntry, validate_name};
use crate::{Inner, PluginsService, io_err, load_disabled, lock, now_ms, save_disabled, topics};

/// Log lines kept per plugin.
const LOG_LINES: usize = 500;
/// Failures in a row (each within [`STABLE_AFTER`] of its start) before giving up.
pub const MAX_QUICK_FAILURES: u32 = 5;
/// A process that ran this long counts as healthy; its failure counter starts over.
const STABLE_AFTER: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// One installed plugin (or a broken folder in `data/plugins`).
pub struct PluginState {
    pub dir: PathBuf,
    /// `Err` = the folder has no valid `plugin.json`.
    pub manifest: Result<Manifest, String>,
    pub enabled: bool,
    pub status: Status,
    /// The token of the running process; `None` = no connection is accepted.
    pub token: Option<String>,
    pub pid: Option<u32>,
    /// Restarts since it was last started by the user.
    pub restarts: u32,
    pub logs: VecDeque<LogLine>,
    /// Kills the current supervisor and its process.
    stop: Option<CancellationToken>,
    /// Grows with every start/stop; a supervisor whose number is old stops touching the state.
    generation: u64,
}

/// What a plugin is doing right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Switched off by the user.
    Disabled,
    /// Enabled; the browser API is not up yet.
    Waiting,
    Running,
    /// Failed; a restart is scheduled.
    Restarting,
    /// The script ended by itself with exit code 0 (no event handlers left).
    Exited,
    /// Failed too often in a row; enable or reload to try again.
    Crashed,
    /// Bad manifest or no script host binary.
    Error,
}

/// One line of plugin output.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LogLine {
    pub ts_ms: u64,
    /// `stdout`, `stderr` or `system` (lines from the plugin manager itself).
    pub stream: &'static str,
    pub line: String,
}

/// A plugin as `plugins.list` shows it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PluginInfo {
    pub name: String,
    pub dir: String,
    pub enabled: bool,
    pub status: Status,
    /// Why the plugin is in `error` / `crashed` (manifest problem, last stderr line, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub restarts: u32,
    /// The manifest (missing when it could not be read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
    /// Permissions that amount to full access (see `manifest::Manifest::warnings`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl PluginState {
    fn new(dir: PathBuf, manifest: Result<Manifest, String>, enabled: bool) -> Self {
        let status = match (&manifest, enabled) {
            (Err(_), _) => Status::Error,
            (Ok(_), false) => Status::Disabled,
            (Ok(_), true) => Status::Waiting,
        };
        PluginState {
            dir,
            manifest,
            enabled,
            status,
            token: None,
            pid: None,
            restarts: 0,
            logs: VecDeque::new(),
            stop: None,
            generation: 0,
        }
    }

    fn info(&self, name: &str) -> PluginInfo {
        let last_error = || {
            self.logs
                .iter()
                .rev()
                .find(|l| l.stream != "stdout")
                .map(|l| l.line.clone())
        };
        PluginInfo {
            name: name.to_string(),
            dir: self.dir.display().to_string(),
            enabled: self.enabled,
            status: self.status.clone(),
            error: match (&self.manifest, &self.status) {
                (Err(e), _) => Some(e.clone()),
                (_, Status::Error | Status::Crashed | Status::Restarting) => last_error(),
                _ => None,
            },
            pid: self.pid,
            restarts: self.restarts,
            manifest: self.manifest.as_ref().ok().cloned(),
            warnings: self
                .manifest
                .as_ref()
                .map(Manifest::warnings)
                .unwrap_or_default(),
        }
    }

    fn log(&mut self, stream: &'static str, line: impl Into<String>) {
        if self.logs.len() == LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(LogLine {
            ts_ms: now_ms(),
            stream,
            line: line.into(),
        });
    }
}

impl PluginAuth for PluginsService {
    fn authenticate(&self, plugin: &str, token: &str) -> Option<PluginGrant> {
        let plugins = lock(&self.inner.plugins);
        let state = plugins.get(plugin)?;
        let current = state.token.as_deref()?;
        if !crate::constant_time_eq(current.as_bytes(), token.as_bytes()) {
            return None;
        }
        let manifest = state.manifest.as_ref().ok()?;
        Some(PluginGrant {
            name: manifest.name.clone(),
            commands: manifest.permissions.clone(),
            events: manifest.events.clone(),
        })
    }
}

/// Copy a plugin folder (files and sub-folders, no symlinks) into place.
fn copy_dir(from: &Path, to: &Path, depth: u32) -> Result<(), CmdError> {
    if depth > 8 {
        return Err(CmdError::invalid_params(format!(
            "{}: folders nested too deeply",
            from.display()
        )));
    }
    std::fs::create_dir_all(to).map_err(|e| io_err(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| io_err(from, e))? {
        let entry = entry.map_err(|e| io_err(from, e))?;
        let kind = entry.file_type().map_err(|e| io_err(&entry.path(), e))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target, depth + 1)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).map_err(|e| io_err(&entry.path(), e))?;
        }
    }
    Ok(())
}

impl Inner {
    /// Re-read `data/plugins`: new folders appear, deleted ones go away (and are stopped).
    pub fn rescan_plugins(&self) {
        let disabled = load_disabled(&self.config.plugins_dir);
        let mut found = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.config.plugins_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !entry.path().is_dir() {
                    continue;
                }
                found.push((name, entry.path()));
            }
        }
        let mut plugins = lock(&self.plugins);
        plugins.retain(|name, state| {
            let keep = found.iter().any(|(n, _)| n == name);
            if !keep && let Some(stop) = state.stop.take() {
                stop.cancel();
            }
            keep
        });
        for (name, dir) in found {
            if plugins.contains_key(&name) {
                continue;
            }
            let manifest = read_manifest(&name, &dir);
            let enabled = !disabled.contains(&name);
            plugins.insert(name, PluginState::new(dir, manifest, enabled));
        }
    }

    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        lock(&self.plugins)
            .iter()
            .map(|(name, state)| state.info(name))
            .collect()
    }

    pub fn plugin_info(&self, name: &str) -> Result<PluginInfo, CmdError> {
        lock(&self.plugins)
            .get(name)
            .map(|s| s.info(name))
            .ok_or_else(|| not_installed(name))
    }

    /// Copy the folder at `source` into `data/plugins/<name>` (validated first).
    pub fn install(
        self: &Arc<Self>,
        source: &Path,
        enable: bool,
        replace: bool,
    ) -> Result<PluginInfo, CmdError> {
        let manifest = Manifest::load(source)?;
        let name = manifest.name.clone();
        let target = self.config.plugins_dir.join(&name);
        let same_folder = std::fs::canonicalize(source).ok() == std::fs::canonicalize(&target).ok();
        if !same_folder {
            if target.exists() {
                if !replace {
                    return Err(CmdError::invalid_params(format!(
                        "plugin {name} is already installed"
                    ))
                    .with_hint("Pass replace: true to overwrite it, or plugins.remove it first."));
                }
                self.stop(&name);
                std::fs::remove_dir_all(&target).map_err(|e| io_err(&target, e))?;
            }
            copy_dir(source, &target, 0)?;
        }
        self.stop(&name);
        lock(&self.plugins).remove(&name);
        self.set_enabled_flag(&name, enable)?;
        self.rescan_plugins();
        if enable {
            self.launch(&name);
        }
        self.plugin_info(&name)
    }

    /// Switch a plugin on (and start it) or off (and stop it). Remembered across restarts.
    pub fn set_enabled(
        self: &Arc<Self>,
        name: &str,
        enabled: bool,
    ) -> Result<PluginInfo, CmdError> {
        {
            let mut plugins = lock(&self.plugins);
            let state = plugins.get_mut(name).ok_or_else(|| not_installed(name))?;
            if let Err(err) = &state.manifest
                && enabled
            {
                return Err(CmdError::invalid_params(format!(
                    "plugin {name} cannot run: {err}"
                )));
            }
            state.enabled = enabled;
        }
        self.set_enabled_flag(name, enabled)?;
        self.stop(name);
        if enabled {
            // A fresh start: forget earlier crashes.
            if let Some(state) = lock(&self.plugins).get_mut(name) {
                state.restarts = 0;
            }
            self.launch(name);
        }
        self.plugin_info(name)
    }

    /// Stop the plugin and delete its folder.
    pub fn remove(&self, name: &str) -> Result<bool, CmdError> {
        validate_name(name)?;
        self.stop(name);
        let existed = lock(&self.plugins).remove(name).is_some();
        let dir = self.config.plugins_dir.join(name);
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir).map_err(|e| io_err(&dir, e))?;
        }
        let mut disabled = load_disabled(&self.config.plugins_dir);
        if disabled.remove(name) {
            save_disabled(&self.config.plugins_dir, &disabled)?;
        }
        Ok(existed)
    }

    /// Re-read one plugin's manifest from disk and restart it (or rescan everything).
    pub fn reload(self: &Arc<Self>, name: Option<&str>) -> Result<Vec<PluginInfo>, CmdError> {
        let names: Vec<String> = match name {
            Some(name) => {
                if !lock(&self.plugins).contains_key(name) {
                    // Maybe it was just copied into data/plugins by hand.
                    self.rescan_plugins();
                }
                self.plugin_info(name)?;
                vec![name.to_string()]
            }
            None => {
                self.rescan_plugins();
                lock(&self.plugins).keys().cloned().collect()
            }
        };
        for name in &names {
            self.stop(name);
            let mut plugins = lock(&self.plugins);
            if let Some(state) = plugins.get_mut(name) {
                state.manifest = read_manifest(name, &state.dir);
                state.restarts = 0;
                state.status = match (&state.manifest, state.enabled) {
                    (Err(_), _) => Status::Error,
                    (Ok(_), false) => Status::Disabled,
                    (Ok(_), true) => Status::Waiting,
                };
            }
            drop(plugins);
            self.launch(name);
        }
        Ok(names
            .iter()
            .filter_map(|n| self.plugin_info(n).ok())
            .collect())
    }

    pub fn logs(&self, name: &str, limit: usize) -> Result<Vec<LogLine>, CmdError> {
        let plugins = lock(&self.plugins);
        let state = plugins.get(name).ok_or_else(|| not_installed(name))?;
        let skip = state.logs.len().saturating_sub(limit);
        Ok(state.logs.iter().skip(skip).cloned().collect())
    }

    /// Palette entries of the enabled plugins: `(plugin, entry)`.
    pub fn palette(&self) -> Vec<(String, PaletteEntry)> {
        lock(&self.plugins)
            .iter()
            .filter(|(_, s)| s.enabled)
            .filter_map(|(name, s)| s.manifest.as_ref().ok().map(|m| (name, m)))
            .flat_map(|(name, m)| m.ui.palette.iter().map(|e| (name.clone(), e.clone())))
            .collect()
    }

    /// Send `plugin.command {plugin, id}`; true when the plugin is running to receive it.
    pub fn palette_run(&self, plugin: &str, id: &str) -> Result<bool, CmdError> {
        let running = {
            let plugins = lock(&self.plugins);
            let state = plugins.get(plugin).ok_or_else(|| not_installed(plugin))?;
            let declared = state
                .manifest
                .as_ref()
                .is_ok_and(|m| m.ui.palette.iter().any(|e| e.id == id));
            if !declared {
                return Err(CmdError::new(
                    ErrorCode::NotFound,
                    format!("plugin {plugin} has no palette entry {id:?}"),
                ));
            }
            state.status == Status::Running
        };
        self.events.emit(
            topics::PLUGIN_COMMAND,
            json!({ "plugin": plugin, "id": id }),
        );
        Ok(running)
    }

    /// Running plugins' userscripts: `(plugin, file, plugin WS URL)`.
    pub fn plugin_userscripts(&self) -> Vec<(String, PathBuf, String)> {
        let base = self.ws_url();
        lock(&self.plugins)
            .iter()
            .filter(|(_, s)| s.status == Status::Running)
            .filter_map(|(name, s)| {
                let manifest = s.manifest.as_ref().ok()?;
                let token = s.token.as_ref()?;
                let url = plugin_ws_url(&base, name, token);
                Some(
                    manifest
                        .userscripts
                        .iter()
                        .map(|file| (name.clone(), s.dir.join(file), url.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    }

    fn set_enabled_flag(&self, name: &str, enabled: bool) -> Result<(), CmdError> {
        let mut disabled = load_disabled(&self.config.plugins_dir);
        let changed = if enabled {
            disabled.remove(name)
        } else {
            disabled.insert(name.to_string())
        };
        if changed {
            save_disabled(&self.config.plugins_dir, &disabled)?;
        }
        Ok(())
    }

    /// Stop the plugin's process and revoke its token.
    pub fn stop(&self, name: &str) {
        let mut plugins = lock(&self.plugins);
        let Some(state) = plugins.get_mut(name) else {
            return;
        };
        state.generation += 1;
        state.token = None;
        state.pid = None;
        if let Some(stop) = state.stop.take() {
            stop.cancel();
            state.log("system", "stopped");
        }
        if state.manifest.is_ok() {
            state.status = if state.enabled {
                Status::Waiting
            } else {
                Status::Disabled
            };
        }
    }

    /// Start the supervisor of an enabled, valid plugin (no-op before [`crate::PluginsService::start`]).
    pub fn launch(self: &Arc<Self>, name: &str) {
        if !self.is_started() || self.shutdown.is_cancelled() {
            return;
        }
        let token = match crate::random_hex(32) {
            Ok(token) => token,
            Err(err) => {
                tracing::error!(%err, "cannot create a plugin token");
                return;
            }
        };
        let mut plugins = lock(&self.plugins);
        let Some(state) = plugins.get_mut(name) else {
            return;
        };
        if !state.enabled || state.manifest.is_err() || state.stop.is_some() {
            return;
        }
        state.generation += 1;
        state.token = Some(token);
        let stop = self.shutdown.child_token();
        state.stop = Some(stop.clone());
        let generation = state.generation;
        drop(plugins);
        tokio::spawn(supervise(self.clone(), name.to_string(), generation, stop));
    }

    /// Run `f` on the plugin's state if `generation` is still current.
    fn with_current<R>(
        &self,
        name: &str,
        generation: u64,
        f: impl FnOnce(&mut PluginState) -> R,
    ) -> Option<R> {
        let mut plugins = lock(&self.plugins);
        let state = plugins.get_mut(name)?;
        (state.generation == generation).then(|| f(state))
    }

    fn set_status(&self, name: &str, generation: u64, status: Status, note: Option<String>) {
        let changed = self.with_current(name, generation, |state| {
            if let Some(note) = &note {
                state.log("system", note.clone());
            }
            if matches!(status, Status::Crashed | Status::Error | Status::Exited) {
                state.token = None;
                state.pid = None;
                state.stop = None;
            }
            let changed = state.status != status;
            state.status = status.clone();
            changed
        });
        if changed == Some(true) {
            self.events.emit(
                topics::PLUGIN_STATUS,
                json!({ "name": name, "status": status, "note": note }),
            );
        }
    }
}

/// `ws://…/v1/ws?plugin=<name>&token=<token>` (name and token need no URL escaping).
pub fn plugin_ws_url(base: &str, name: &str, token: &str) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}plugin={name}&token={token}")
}

fn read_manifest(folder: &str, dir: &Path) -> Result<Manifest, String> {
    if !dir.join(MANIFEST_FILE).is_file() {
        return Err(format!("no {MANIFEST_FILE} in {}", dir.display()));
    }
    let manifest = Manifest::load(dir).map_err(|e| e.message)?;
    if manifest.name != folder {
        return Err(format!(
            "folder {folder:?} holds plugin {:?}; the folder must be named like the plugin",
            manifest.name
        ));
    }
    Ok(manifest)
}

fn not_installed(name: &str) -> CmdError {
    CmdError::new(
        ErrorCode::NotFound,
        format!("plugin {name} is not installed"),
    )
    .with_hint("See plugins.list; install one with plugins.install {path}.")
}

/// How one plugin process ended.
enum Ended {
    Cancelled,
    Exited(Option<i32>),
    SpawnFailed(String),
}

/// Keep the plugin running until it is stopped, exits cleanly or fails too often.
async fn supervise(inner: Arc<Inner>, name: String, generation: u64, stop: CancellationToken) {
    let mut failures = 0u32;
    loop {
        let exe = match bmcp_jobs::host::locate_host(inner.config.host_exe.as_deref()) {
            Ok(exe) => exe,
            Err(err) => {
                inner.set_status(&name, generation, Status::Error, Some(err.message));
                return;
            }
        };
        let Some((main, token)) = inner
            .with_current(&name, generation, |s| {
                let main = s.manifest.as_ref().ok().map(|m| s.dir.join(&m.main));
                main.zip(s.token.clone())
            })
            .flatten()
        else {
            return; // stopped meanwhile
        };
        let url = plugin_ws_url(&inner.ws_url(), &name, &token);
        let started = Instant::now();
        match run_process(&inner, &name, generation, &exe, &main, &url, &stop).await {
            Ended::Cancelled => return,
            Ended::SpawnFailed(message) => {
                inner.set_status(&name, generation, Status::Error, Some(message));
                return;
            }
            Ended::Exited(Some(0)) => {
                inner.set_status(
                    &name,
                    generation,
                    Status::Exited,
                    Some("exited (code 0)".into()),
                );
                return;
            }
            Ended::Exited(code) => {
                if started.elapsed() >= STABLE_AFTER {
                    failures = 0;
                }
                failures += 1;
                let code = code.map_or("killed".to_string(), |c| format!("exit code {c}"));
                if failures >= MAX_QUICK_FAILURES {
                    let note = format!("{code}; failed {failures} times in a row, giving up");
                    inner.set_status(&name, generation, Status::Crashed, Some(note));
                    return;
                }
                let pause = (Duration::from_secs(1) * 2u32.pow(failures - 1)).min(MAX_BACKOFF);
                let note = format!("{code}; restarting in {}s", pause.as_secs());
                inner.with_current(&name, generation, |s| s.restarts += 1);
                inner.set_status(&name, generation, Status::Restarting, Some(note));
                tokio::select! {
                    _ = tokio::time::sleep(pause) => {}
                    _ = stop.cancelled() => return,
                }
            }
        }
    }
}

/// Start the host once and wait for it; its output goes into the plugin's log.
async fn run_process(
    inner: &Arc<Inner>,
    name: &str,
    generation: u64,
    exe: &Path,
    main: &Path,
    ws_url: &str,
    stop: &CancellationToken,
) -> Ended {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("run")
        .arg(main)
        .arg("--ws")
        .arg(ws_url)
        .arg("--input")
        .arg(format!("plugin={name}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // The release browser is a GUI app: no console window per plugin.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return Ended::SpawnFailed(format!("cannot start {}: {err}", exe.display())),
    };
    let pid = child.id();
    inner.with_current(name, generation, |s| s.pid = pid);
    inner.set_status(
        name,
        generation,
        Status::Running,
        Some(match pid {
            Some(pid) => format!("started (pid {pid})"),
            None => "started".to_string(),
        }),
    );

    let out = tokio::spawn(pump(
        inner.clone(),
        name.to_string(),
        generation,
        "stdout",
        child.stdout.take(),
    ));
    let err = tokio::spawn(pump(
        inner.clone(),
        name.to_string(),
        generation,
        "stderr",
        child.stderr.take(),
    ));
    let ended = tokio::select! {
        status = child.wait() => Ended::Exited(status.ok().and_then(|s| s.code())),
        _ = stop.cancelled() => {
            let _ = child.kill().await;
            Ended::Cancelled
        }
    };
    // Let the readers drain what the process wrote before it ended.
    let _ = out.await;
    let _ = err.await;
    ended
}

/// Copy lines from a pipe into the plugin's log.
async fn pump(
    inner: Arc<Inner>,
    name: String,
    generation: u64,
    stream: &'static str,
    pipe: Option<impl AsyncRead + Unpin>,
) {
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(plugin = %name, stream, "{line}");
        inner.with_current(&name, generation, |s| s.log(stream, line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_gets_the_plugin_query() {
        assert_eq!(
            plugin_ws_url("ws://127.0.0.1:1/v1/ws", "hello", "ab12"),
            "ws://127.0.0.1:1/v1/ws?plugin=hello&token=ab12"
        );
        assert_eq!(
            plugin_ws_url("ws://h/v1/ws?x=1", "a", "b"),
            "ws://h/v1/ws?x=1&plugin=a&token=b"
        );
    }
}
