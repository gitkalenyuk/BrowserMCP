//! CEF integration. Rule: no `cef::` type ever leaves this crate.
//!
//! Startup sequence (see [`start`]):
//! 1. The same executable is launched by Chromium for renderer/GPU/utility sub-processes
//!    with a `--type=...` switch. [`start`] detects this and runs the sub-process.
//! 2. Otherwise it is the browser process: initialize CEF, open the shell window and run
//!    the message loop on the main thread (the CEF UI thread) until every window closed.

mod shell;

use bmcp_core::{DataDir, ProfileId};
use cef::{CefString, Settings, api_hash, args::Args, execute_process, sys};

/// Which CEF runtime style windows use. Decided for good in spike S1;
/// Chrome style gives Chrome's own permission/download/print UI, Alloy gives full control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Chrome,
    Alloy,
}

/// Options for the browser process.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub start_url: String,
    pub data: DataDir,
    pub profile: ProfileId,
    pub style: Style,
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
    let code = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
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

        let mut app = shell::ShellApp::new(std::sync::Arc::new(shell::StartConfig {
            url: opts.start_url,
            style: opts.style,
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
        cef::shutdown();
        tracing::info!("CEF shut down");
        Ok(())
    }
}

fn cef_path(path: &std::path::Path) -> CefString {
    CefString::from(path.to_string_lossy().as_ref())
}
