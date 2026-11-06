//! CEF integration. Rule: no `cef::` type ever leaves this crate.
//!
//! Startup sequence (see [`start`]):
//! 1. The same executable is launched by Chromium for renderer/GPU/utility sub-processes
//!    with a `--type=...` switch. [`start`] detects this and runs the sub-process (with a
//!    small App that registers the `bmcp://` scheme, which every process must know).
//! 2. Otherwise it is the browser process: initialize CEF, open the shell window and run
//!    the message loop on the main thread (the CEF UI thread) until every window closed.

mod backend;
mod bridge;
mod chrome;
mod scheme;
mod shell;
mod switches;
mod ui_files;

pub use backend::CefBackend;
pub use shell::ReadyCallback;

use std::path::PathBuf;

use bmcp_core::{DataDir, EventBus, ProfileId};
use cef::{CefString, Settings, api_hash, args::Args, execute_process, sys};

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

        let settings = Settings {
            // Windows sandbox needs a separate bootstrap executable; tracked as a known risk.
            no_sandbox: 1,
            root_cache_path: cef_path(&opts.data.profiles()),
            cache_path: cef_path(&profile_dir),
            log_file: cef_path(&opts.data.logs().join("cef.log")),
            persist_session_cookies: 1,
            ..Default::default()
        };

        bridge::set_events(opts.events);
        scheme::configure(&opts.ui);

        let mut app = shell::ShellApp::new(std::sync::Arc::new(shell::StartConfig {
            url: opts.start_url,
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
    CefString::from(path.to_string_lossy().as_ref())
}
