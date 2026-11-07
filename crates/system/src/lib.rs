//! Show files and folders in Windows Explorer:
//!
//! * `system.reveal_path {path}` — opens Explorer on the containing folder with the file (or
//!   folder) selected: `explorer.exe /select,"<path>"`;
//! * `system.open_path {path}` — opens a folder: `explorer.exe "<path>"`.
//!
//! Safety rules (both commands):
//!
//! * the path must start with a drive letter (`C:\…`): UNC and device paths (`\\host\share`,
//!   `\\.\pipe`) are refused before the file system is touched, because even checking a UNC
//!   path makes an SMB connection (and would leak the user's NTLM hash to that host);
//! * the path must be absolute and exist; it is resolved with `canonicalize` (which follows
//!   `..`, links and junctions) **before** the check, so `<data>\..\x` or a junction out of the
//!   folder is refused;
//! * the resolved path must be inside the BrowserMCP data folder ([`DataDir`] in the registry
//!   extensions) or the user's Downloads folder;
//! * `open_path` only takes folders: Explorer *runs* a file it is asked to open, so files go
//!   through `reveal_path`, which only selects them;
//! * Explorer is started directly from `%SystemRoot%\explorer.exe` with one argument. No shell,
//!   no `cmd /c`, no other program.
//!
//! Tests replace Explorer with a fake [`Opener`] put into the registry extensions
//! ([`OpenerExt`]), so they never open windows on the desktop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bmcp_core::{CmdError, Command, Ctx, DataDir, ErrorCode, Meta, Registry, Retry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Register `system.reveal_path` and `system.open_path`.
pub fn register(registry: &mut Registry) {
    registry.add::<RevealPath>().add::<OpenPath>();
}

// ---------------------------------------------------------------------------------------------
// Path checks
// ---------------------------------------------------------------------------------------------

/// The folders the commands may show: the data folder (when the app registered one), the
/// user's Downloads folder and the app's [`ExtraRoots`].
pub fn allowed_roots(ctx: &Ctx) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(data) = ctx.extensions.get::<DataDir>() {
        roots.push(data.root().to_path_buf());
    }
    roots.extend(downloads_dir());
    if let Some(extra) = ctx.extensions.get::<ExtraRoots>() {
        roots.extend((extra.0)());
    }
    roots
}

/// More folders the app lets these commands show, asked on every call (they can change while
/// the browser runs): the download folder chosen in Settings and the folder of the exe (its
/// license files). Put into the registry extensions by the app.
#[derive(Clone)]
pub struct ExtraRoots(pub Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>);

/// `%USERPROFILE%\Downloads` (the default location; a moved Downloads folder is not followed).
pub fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join("Downloads"))
}

/// Resolve `path` and check it against `roots`. Returns the resolved path in its plain form
/// (`C:\…`, not `\\?\C:\…`, which Explorer does not understand).
pub fn check_path(path: &str, roots: &[PathBuf]) -> Result<PathBuf, CmdError> {
    let raw = Path::new(path.trim());
    if raw.as_os_str().is_empty() {
        return Err(CmdError::invalid_params("`path` must not be empty"));
    }
    if !raw.is_absolute() {
        return Err(CmdError::invalid_params(format!(
            "`path` must be absolute, got {path:?}"
        )));
    }
    if !on_drive(raw) {
        return Err(CmdError::invalid_params(format!(
            "`path` must be on a drive (C:\\…), not a network or device path: {path:?}"
        )));
    }
    let resolved = raw.canonicalize().map_err(|err| {
        CmdError::new(
            ErrorCode::NotFound,
            format!("{} does not exist ({err})", raw.display()),
        )
        .with_retry(Retry::No)
    })?;
    let inside = roots
        .iter()
        // A root that does not exist (no Downloads folder) simply allows nothing.
        .filter_map(|root| root.canonicalize().ok())
        // `starts_with` compares whole components: `C:\data2` is not inside `C:\data`.
        .any(|root| resolved.starts_with(&root));
    let resolved_plain = plain_path(resolved);
    if !inside {
        return Err(CmdError::invalid_params(format!(
            "{} is outside the BrowserMCP data folder and the Downloads folder",
            resolved_plain.display()
        ))
        .with_hint("only files in the data folder or Downloads can be shown"));
    }
    Ok(resolved_plain)
}

/// True for `C:\…` and `\\?\C:\…` (decided from the text alone; nothing is opened).
#[cfg(windows)]
fn on_drive(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    match path.components().next() {
        Some(Component::Prefix(prefix)) => {
            matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        }
        _ => false,
    }
}

/// Elsewhere every absolute path is local.
#[cfg(not(windows))]
fn on_drive(_path: &Path) -> bool {
    true
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\server\share` → `\\server\share`; anything else unchanged.
fn plain_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

// ---------------------------------------------------------------------------------------------
// Explorer
// ---------------------------------------------------------------------------------------------

/// Shows paths to the user. [`Explorer`] in the app; a fake in tests.
pub trait Opener: Send + Sync {
    /// Open the folder containing `path` with `path` selected.
    fn reveal(&self, path: &Path) -> std::io::Result<()>;
    /// Open the folder `path`.
    fn open_folder(&self, path: &Path) -> std::io::Result<()>;
}

/// Put into the registry extensions to replace [`Explorer`] (tests).
pub struct OpenerExt(pub Arc<dyn Opener>);

/// Windows Explorer.
pub struct Explorer;

impl Opener for Explorer {
    fn reveal(&self, path: &Path) -> std::io::Result<()> {
        // Explorer has its own command-line parser: `/select,` must be followed directly by
        // the quoted path. Paths cannot contain `"` on Windows (checked again in `explorer`).
        explorer(&format!("/select,\"{}\"", path.display()), path)
    }

    fn open_folder(&self, path: &Path) -> std::io::Result<()> {
        explorer(&format!("\"{}\"", path.display()), path)
    }
}

/// Start `%SystemRoot%\explorer.exe <arg>` (one raw argument, no shell) and reap it in the
/// background. Explorer's exit code is meaningless (often 1 on success), so it is not checked.
#[cfg(windows)]
fn explorer(arg: &str, path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    if path.to_string_lossy().contains('"') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path contains a quote",
        ));
    }
    let windows = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut child = std::process::Command::new(windows.join("explorer.exe"))
        .raw_arg(arg)
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(not(windows))]
fn explorer(_arg: &str, _path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "showing files is only implemented on Windows",
    ))
}

fn opener(ctx: &Ctx) -> Arc<dyn Opener> {
    match ctx.extensions.get::<OpenerExt>() {
        Some(ext) => ext.0.clone(),
        None => Arc::new(Explorer),
    }
}

fn launch_error(err: std::io::Error) -> CmdError {
    if err.kind() == std::io::ErrorKind::Unsupported {
        CmdError::new(ErrorCode::NotImplemented, err.to_string())
    } else {
        CmdError::internal(format!("cannot start Explorer: {err}"))
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathArgs {
    /// Absolute path inside the BrowserMCP data folder or the Downloads folder.
    pub path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PathOutput {
    /// The resolved path that was shown.
    pub path: String,
}

/// `system.reveal_path`.
pub struct RevealPath;

impl Command for RevealPath {
    const NAME: &'static str = "system.reveal_path";
    type Args = PathArgs;
    type Output = PathOutput;

    fn meta() -> Meta {
        Meta::new(
            "Show a file or folder in Windows Explorer, selected in its folder. Only paths \
             inside the BrowserMCP data folder or Downloads. Returns {path}.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PathArgs) -> Result<PathOutput, CmdError> {
        let path = check_path(&args.path, &allowed_roots(&ctx))?;
        opener(&ctx).reveal(&path).map_err(launch_error)?;
        tracing::info!(path = %path.display(), "revealed in Explorer");
        Ok(PathOutput {
            path: path.display().to_string(),
        })
    }
}

/// `system.open_path`.
pub struct OpenPath;

impl Command for OpenPath {
    const NAME: &'static str = "system.open_path";
    type Args = PathArgs;
    type Output = PathOutput;

    fn meta() -> Meta {
        Meta::new(
            "Open a folder in Windows Explorer (folders only; use system.reveal_path for a \
             file). Only folders inside the BrowserMCP data folder or Downloads. Returns {path}.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PathArgs) -> Result<PathOutput, CmdError> {
        let path = check_path(&args.path, &allowed_roots(&ctx))?;
        if !path.is_dir() {
            return Err(
                CmdError::invalid_params(format!("{} is not a folder", path.display()))
                    .with_hint("use system.reveal_path to show a file"),
            );
        }
        opener(&ctx).open_folder(&path).map_err(launch_error)?;
        tracing::info!(path = %path.display(), "opened in Explorer");
        Ok(PathOutput {
            path: path.display().to_string(),
        })
    }
}

#[cfg(test)]
mod tests;
