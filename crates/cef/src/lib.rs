//! CEF integration. Rule: no `cef::` type ever leaves this crate.
//!
//! Startup sequence (see [`start`]):
//! 1. The same executable is launched by Chromium for renderer/GPU/utility sub-processes
//!    with a `--type=...` switch. [`start`] detects this and runs the sub-process (with a
//!    small App that registers the `bmcp://` scheme, which every process must know).
//! 2. Otherwise it is the browser process: initialize CEF, open the shell window and run
//!    the message loop on the main thread (the CEF UI thread) until every window closed.
//!    Headless (`LaunchOptions::headless`): no window at all, the first tab is off-screen and
//!    the loop runs until `quit` (see `osr.rs`).

mod backend;
mod bridge;
mod cdp_port;
mod chrome;
mod clipboard;
mod cookies;
mod devtools;
mod downloads;
mod hooks;
mod languages;
mod menu;
mod net;
mod osr;
mod profiles;
mod prompts;
mod scheme;
mod session;
mod shell;
mod switches;
mod ui_files;
mod zoom;

pub use backend::CefBackend;
pub use hooks::set_tab_preparer;
pub use osr::HeadlessOptions;
pub use shell::ReadyCallback;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use bmcp_adblock::ResourceGate;
use bmcp_core::{DataDir, EventBus, ProfileId};
use bmcp_profiles::PermissionStore;
use cef::{CefString, Settings, api_hash, args::Args, execute_process, sys};

/// Site permission rules (shared with the `permissions.*` commands).
static PERMISSIONS: OnceLock<PermissionStore> = OnceLock::new();

pub(crate) fn permission_store() -> Option<&'static PermissionStore> {
    PERMISSIONS.get()
}

/// Which CEF runtime style windows use.
///
/// Alloy is the default (spike S1/S2 decision, see docs/spikes/S1-S2-S3.md): it gives us
/// full control over our own UI, and closing the last window exits cleanly. In Chrome
/// style `quit_message_loop()` did not end the loop with CEF 156, so the process hung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    Chrome,
    #[default]
    Alloy,
}

/// How a browser window is composed (spike S1, see docs/spikes/S1-S2-S3.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowLayout {
    /// The Svelte chrome (`bmcp://ui/`) around the tabs' pages, Ctrl+K palette overlay.
    #[default]
    Chrome,
    /// Phase 0: the page fills the window, no UI of our own; every tab is a window.
    Single,
}

/// Where the browser's own UI pages come from and where they find the API.
#[derive(Debug, Clone, Default)]
pub struct UiOptions {
    /// `ws://127.0.0.1:<port>/v1/ws`, handed to every `bmcp://ui/` page.
    pub api_ws: Option<String>,
    /// Serve `bmcp://ui/` from this folder instead of the embedded build (`--ui-dir`).
    pub dir: Option<PathBuf>,
    /// Load the chrome and palette from a Vite dev server instead (`--ui-dev
    /// http://localhost:5173`, needs `BMCP_DEV_UI=1` for the API origin check).
    pub dev_url: Option<String>,
}

/// Options for the browser process.
pub struct LaunchOptions {
    pub start_url: String,
    pub data: DataDir,
    pub profile: ProfileId,
    pub style: Style,
    pub layout: WindowLayout,
    pub ui: UiOptions,
    /// Bus for `tab.*` events. Pass the same bus to the API server so WS clients (the UI)
    /// receive them.
    pub events: EventBus,
    /// Runs on the UI thread once the first window exists. Hand the backend to async
    /// code (API servers, scripts) from here; do not block.
    pub on_ready: Option<ReadyCallback>,
    /// Asked for every network request of web pages (adblock + site policies, see
    /// `crates/adblock/README.md`). `None` = nothing is filtered in the network stack.
    pub resource_gate: Option<Arc<dyn ResourceGate>>,
    /// Site permission rules; `None` = every web permission request is denied.
    pub permissions: Option<PermissionStore>,
    /// Open Chromium's remote debugging port (for Playwright/Puppeteer `connectOverCDP`).
    /// Off by default; the caller must not combine it with stealth profiles (a debugging
    /// port is easy to detect).
    pub cdp_port: Option<u16>,
    /// Re-open the tabs of the last session (`data/runtime/session.json`) instead of
    /// `start_url` (off with `--no-restore`). Tabs of profiles other than the launch one
    /// wait for [`profiles_applied`].
    pub restore_session: bool,
    /// `--headless`: no window or view at all; every tab is an off-screen page of this size,
    /// and the process keeps running without tabs until `system.quit` (see `osr.rs`).
    pub headless: Option<HeadlessOptions>,
}

/// The app applied the stored profile settings (proxies, identities): restored tabs of
/// those profiles may load now (see `session.rs`). Any thread.
pub fn profiles_applied() {
    bridge::post_to_ui(Box::new(session::restore_deferred));
}

/// What [`start`] found out about the current process.
pub enum Process {
    /// A Chromium sub-process ran to completion; exit with this code.
    SubprocessExited(i32),
    /// This is the browser process; call [`Browser::run`].
    Browser(Browser),
}

/// Handle to an un-initialized browser process.
pub struct Browser {
    args: Args,
}

/// Must be the very first thing `main` does.
pub fn start() -> Process {
    // Tell libcef which API version these bindings were generated for.
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = Args::new();
    // Sub-processes need an App too: custom schemes must be registered in every process.
    let mut app = scheme::SchemeApp::new();
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if code >= 0 {
        Process::SubprocessExited(code)
    } else {
        Process::Browser(Browser { args })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("failed to prepare data folder: {0}")]
    Data(#[from] bmcp_core::PathError),
    #[error("CEF failed to initialize (another instance may be using {0})")]
    Initialize(String),
}

impl Browser {
    /// Initialize CEF, open the first window and block until the user closes the last one.
    pub fn run(self, opts: LaunchOptions) -> Result<(), RunError> {
        opts.data.ensure()?;
        let profile_dir = opts.data.profile(&opts.profile);
        // Chromium's own locale and the website languages (data/config/languages.json).
        let languages = languages::startup(&opts.data.config());

        let settings = Settings {
            // Windows sandbox needs a separate bootstrap executable; tracked as a known risk.
            no_sandbox: 1,
            root_cache_path: cef_path(&opts.data.profiles()),
            cache_path: cef_path(&profile_dir),
            log_file: cef_path(&opts.data.logs().join("cef.log")),
            persist_session_cookies: 1,
            // Off-screen tabs (`tabs.open {window: "offscreen"}`) are windowless browsers.
            windowless_rendering_enabled: 1,
            // 0 = no remote debugging port.
            remote_debugging_port: opts.cdp_port.map_or(0, i32::from),
            // Empty = the Windows display language (Chromium's default). The website languages
            // are a pref set per request context (languages.rs: not accept_language_list).
            locale: CefString::from(languages.locale.as_deref().unwrap_or("")),
            ..Default::default()
        };

        bridge::set_events(opts.events);
        scheme::configure(&opts.ui);
        profiles::set_launch(opts.data.root().to_path_buf(), opts.profile.to_string());
        if let Some(gate) = opts.resource_gate {
            net::set_gate(gate);
        }
        if let Some(permissions) = opts.permissions {
            let _ = PERMISSIONS.set(permissions);
        }
        if let Some(headless) = opts.headless {
            osr::configure_headless(headless);
        }
        if let Some(port) = opts.cdp_port {
            cdp_port::configure(port);
            tracing::warn!(
                port,
                "remote debugging port open: any local program can control this browser"
            );
        }

        let mut app = shell::ShellApp::new(std::sync::Arc::new(shell::StartConfig {
            url: opts.start_url,
            restore: opts.restore_session,
            style: opts.style,
            layout: opts.layout,
            on_ready: std::sync::Mutex::new(opts.on_ready),
        }));

        let ok = cef::initialize(
            Some(self.args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        );
        if ok != 1 {
            return Err(RunError::Initialize(opts.data.root().display().to_string()));
        }

        tracing::info!(data = %opts.data.root().display(), "CEF initialized");
        cef::run_message_loop();
        tracing::debug!("message loop exited, shutting CEF down");
        bridge::clear_ui_state();
        cef::shutdown();
        tracing::info!("CEF shut down");
        Ok(())
    }
}

fn cef_path(path: &std::path::Path) -> CefString {
    CefString::from(native_path(path).as_str())
}

/// A path as one string with the platform's own separators. Pitfall (Windows): Chromium
/// compares profile folders as strings — a context's `cache_path` must be a direct child of
/// `root_cache_path` — so `C:/data\profiles\work` (forward slashes from `--data-dir` plus
/// joined backslashes) was refused ("Cannot create profile at path").
pub(crate) fn native_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text.into_owned()
    }
}
