//! BrowserMCP extensions: plugins, userscripts and webhook triggers.
//!
//! * [`manifest`] + [`plugins`] — `data/plugins/<name>/` with `plugin.json` and a long-lived
//!   `main.ts`. Each enabled plugin runs in its own `bmcp-script-host` process, connected over
//!   a restricted WebSocket (`/v1/ws?plugin=<name>&token=<token>`, see
//!   [`bmcp_api::access`]): it may only call the commands its manifest lists and only receives
//!   the events it lists. `browser.on("tab.updated", h)` in the script keeps it running.
//!   Commands `plugins.list/install/enable/disable/remove/reload/logs/palette/palette_run`.
//! * [`matching`] + [`userscripts`] — `data/userscripts/*.ts` with Tampermonkey-style headers
//!   (`// @match https://*.example.com/*`, `// @run-at document-end`). When a tab finishes
//!   loading a matching URL the script runs once with `inputs.tab` / `inputs.url`.
//!   Commands `userscripts.list/save/delete/enable/inject`.
//! * [`webhooks`] — `POST /v1/hooks/{id}` with `X-BMCP-Signature: sha256=<HMAC of the body>`
//!   starts a saved script as a one-row run (`run.start`) and answers `{run_id}`. Commands
//!   `trigger.webhook_set/list/remove` (+ the hidden `trigger.webhook_fire` behind the route).
//!
//! The app creates one [`PluginsService`], puts it (and its [`PluginsService::auth_handle`])
//! into the registry's extensions, calls [`register`], and once the API listens
//! [`PluginsService::attach`] + [`PluginsService::start`]:
//!
//! ```no_run
//! # async fn demo(data: bmcp_core::DataDir, events: bmcp_core::EventBus) {
//! let mut registry = bmcp_core::Registry::new();
//! bmcp_plugins::register(&mut registry);
//! let plugins = bmcp_plugins::PluginsService::open(
//!     bmcp_plugins::PluginsConfig::new(&data, 47800), events);
//! registry.extensions().insert(plugins.clone());
//! registry.extensions().insert(plugins.auth_handle());
//! let registry = std::sync::Arc::new(registry);
//! plugins.attach(&registry);
//! plugins.start("ws://127.0.0.1:47800/v1/ws");
//! # }
//! ```

pub mod commands;
pub mod manifest;
pub mod matching;
pub mod plugins;
pub mod userscripts;
pub mod webhooks;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::Duration;

use bmcp_core::{CmdError, DataDir, EventBus, Registry};
use tokio_util::sync::CancellationToken;

use crate::plugins::PluginState;
use crate::userscripts::UserscriptRuntime;
use crate::webhooks::HookStore;

/// Event topics this crate emits.
pub mod topics {
    /// `{plugin, id}`: a palette entry of the plugin was chosen (delivered to that plugin).
    pub use bmcp_api::access::PLUGIN_COMMAND_TOPIC as PLUGIN_COMMAND;
    /// `{name, status, error?, exit_code?}` whenever a plugin starts, stops or crashes.
    pub const PLUGIN_STATUS: &str = "plugin.status";
    /// `{name, source, tab, url, ok, duration_ms, error?}` after a userscript ran.
    pub const USERSCRIPT_RAN: &str = "userscript.ran";
    /// `{id, script, run_id}` after a webhook started its run.
    pub const WEBHOOK_FIRED: &str = "trigger.webhook_fired";
}

/// Paths and knobs.
#[derive(Debug, Clone)]
pub struct PluginsConfig {
    /// `data/plugins`
    pub plugins_dir: PathBuf,
    /// `data/userscripts`
    pub userscripts_dir: PathBuf,
    /// `data/scripts` (webhook scripts are saved scripts).
    pub scripts_dir: PathBuf,
    /// `data/config/webhooks.json`
    pub webhooks_file: PathBuf,
    /// Explicit `bmcp-script-host`; `None` = search like the jobs runner does.
    pub host_exe: Option<PathBuf>,
    /// Where scripts send `browser.*` calls (plugins get `?plugin=…&token=…` appended).
    pub ws_url: String,
    /// Default limit for one userscript run (`// @timeout <secs>` overrides it).
    pub userscript_timeout: Duration,
}

impl PluginsConfig {
    /// The standard layout inside the portable data folder.
    pub fn new(data: &DataDir, api_port: u16) -> Self {
        PluginsConfig {
            plugins_dir: data.root().join("plugins"),
            userscripts_dir: data.root().join("userscripts"),
            scripts_dir: data.root().join("scripts"),
            webhooks_file: data.config().join("webhooks.json"),
            host_exe: None,
            ws_url: format!("ws://127.0.0.1:{api_port}/v1/ws"),
            userscript_timeout: Duration::from_secs(60),
        }
    }
}

/// The long-lived state behind all commands of this crate. Cheap to clone (one `Arc`).
#[derive(Clone)]
pub struct PluginsService {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub config: PluginsConfig,
    pub ws_url: RwLock<String>,
    pub events: EventBus,
    /// Installed plugins by name (also broken ones, so `plugins.list` can show the error).
    pub plugins: Mutex<BTreeMap<String, PluginState>>,
    pub userscripts: UserscriptRuntime,
    pub hooks: HookStore,
    /// For `trigger.webhook_fire` → `run.start` (set by [`PluginsService::attach`]).
    pub registry: OnceLock<Weak<Registry>>,
    /// True after [`PluginsService::start`]: plugin processes may be launched.
    pub started: std::sync::atomic::AtomicBool,
    /// Cancelled by [`PluginsService::shutdown`]: kills every plugin and userscript process.
    pub shutdown: CancellationToken,
}

impl PluginsService {
    /// Create the folders, read the installed plugins and webhooks. Starts nothing.
    pub fn open(config: PluginsConfig, events: EventBus) -> Self {
        for dir in [&config.plugins_dir, &config.userscripts_dir] {
            if let Err(err) = std::fs::create_dir_all(dir) {
                tracing::warn!(%err, dir = %dir.display(), "cannot create folder");
            }
        }
        let inner = Arc::new(Inner {
            ws_url: RwLock::new(config.ws_url.clone()),
            plugins: Mutex::new(BTreeMap::new()),
            userscripts: UserscriptRuntime::default(),
            hooks: HookStore::open(&config.webhooks_file),
            registry: OnceLock::new(),
            started: std::sync::atomic::AtomicBool::new(false),
            shutdown: CancellationToken::new(),
            events,
            config,
        });
        inner.rescan_plugins();
        PluginsService { inner }
    }

    /// What the API needs to authenticate plugin connections; insert it into the registry's
    /// extensions next to the service itself.
    pub fn auth_handle(&self) -> bmcp_api::access::PluginAuthHandle {
        bmcp_api::access::PluginAuthHandle(Arc::new(self.clone()))
    }

    /// Give webhooks access to the registry (they start runs with `run.start`).
    pub fn attach(&self, registry: &Arc<Registry>) {
        let _ = self.inner.registry.set(Arc::downgrade(registry));
    }

    /// The API is listening at `ws_url`: launch the enabled plugins and start watching page
    /// loads for userscripts. Must be called inside a tokio runtime.
    pub fn start(&self, ws_url: impl Into<String>) {
        *self.inner.ws_url.write().unwrap_or_else(|e| e.into_inner()) = ws_url.into();
        self.inner
            .started
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let names: Vec<String> = lock(&self.inner.plugins).keys().cloned().collect();
        for name in names {
            self.inner.launch(&name);
        }
        tokio::spawn(userscripts::watch(self.inner.clone()));
    }

    /// Kill every plugin and userscript process (browser shutdown).
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }
}

impl Inner {
    pub fn ws_url(&self) -> String {
        self.ws_url
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_started(&self) -> bool {
        self.started.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The registry, for commands that start other commands.
    pub fn registry(&self) -> Result<Arc<Registry>, CmdError> {
        self.registry
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| CmdError::internal("the plugin service is not attached to a registry"))
    }
}

/// Register this crate's commands. The app must also insert a [`PluginsService`] into
/// `registry.extensions()`; without it the commands fail with `INTERNAL`.
pub fn register(registry: &mut Registry) {
    use commands::*;
    registry
        .add::<PluginsList>()
        .add::<PluginsInstall>()
        .add::<PluginsEnable>()
        .add::<PluginsDisable>()
        .add::<PluginsRemove>()
        .add::<PluginsReload>()
        .add::<PluginsLogs>()
        .add::<PluginsPalette>()
        .add::<PluginsPaletteRun>()
        .add::<UserscriptsList>()
        .add::<UserscriptsSave>()
        .add::<UserscriptsDelete>()
        .add::<UserscriptsEnable>()
        .add::<UserscriptsInject>()
        .add::<WebhookSet>()
        .add::<WebhookList>()
        .add::<WebhookRemove>()
        .add::<WebhookFire>();
}

// ------------------------------------------------------------------ small shared helpers

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A poisoned lock only means a panic elsewhere; the data is still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `bytes` random bytes from the OS, as lowercase hex.
pub(crate) fn random_hex(bytes: usize) -> Result<String, CmdError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf)
        .map_err(|e| CmdError::internal(format!("no random numbers from the OS: {e}")))?;
    Ok(hex(&buf))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compare two strings in time that does not depend on where they differ (tokens, HMACs).
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn io_err(path: &Path, err: std::io::Error) -> CmdError {
    CmdError::internal(format!("{}: {err}", path.display()))
}

/// The names switched off by the user, kept in `<dir>/.state.json` as `{"disabled": [...]}`.
pub(crate) fn load_disabled(dir: &Path) -> BTreeSet<String> {
    let path = dir.join(".state.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| serde_json::from_value(v["disabled"].clone()).ok())
        .unwrap_or_default()
}

pub(crate) fn save_disabled(dir: &Path, disabled: &BTreeSet<String>) -> Result<(), CmdError> {
    let path = dir.join(".state.json");
    let text = serde_json::json!({ "disabled": disabled }).to_string();
    std::fs::write(&path, text).map_err(|e| io_err(&path, e))
}
