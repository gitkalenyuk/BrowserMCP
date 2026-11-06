//! Small helpers shared by the commands: CDP calls, refs, the data folder, paths.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bmcp_browser::PerceptionState;
use bmcp_core::{CmdError, Ctx, DataDir, ErrorCode, TabId};
use bmcp_perception::PerceptionError;
use serde_json::{Value, json};

/// One raw CDP call; backend failures become `CmdError`s (`TAB_NOT_FOUND`, `TIMEOUT`, …).
pub async fn cdp(ctx: &Ctx, tab: TabId, method: &str, params: Value) -> Result<Value, CmdError> {
    Ok(ctx.backend.cdp(tab, method.to_string(), params).await?)
}

/// Main frame of the tab: `(frame id, loader id)` from `Page.getFrameTree`.
pub async fn main_frame(ctx: &Ctx, tab: TabId) -> Result<(String, String), CmdError> {
    let tree = cdp(ctx, tab, "Page.getFrameTree", json!({})).await?;
    let frame = &tree["frameTree"]["frame"];
    let text = |key: &str| frame[key].as_str().unwrap_or_default().to_string();
    Ok((text("id"), text("loaderId")))
}

/// A ref from `page_snapshot` / `page_find` (`@4:e3`) → its DOM node (`backendNodeId`).
///
/// Uses the browser crate's per-tab ref registry, exactly like `page_act`: if the tab loaded
/// a new document since the snapshot, the ref is stale (`STALE_REF`).
pub async fn resolve_ref(ctx: &Ctx, tab: TabId, target: &str) -> Result<i64, CmdError> {
    let (_, loader) = main_frame(ctx, tab).await?;
    let tab_state = PerceptionState::from_ctx(ctx).tab(tab);
    let mut state = tab_state.lock().await;
    // Same rule as `bmcp_browser`'s `sync_document`: another loader = another document.
    if state.loader_id.as_ref().is_some_and(|old| *old != loader) {
        state.registry.new_document();
    }
    state.loader_id = Some(loader);
    state.registry.resolve(target).map_err(perception_error)
}

/// Perception errors (bad/stale ref) → command errors with advice.
fn perception_error(err: PerceptionError) -> CmdError {
    let message = err.to_string();
    match err {
        PerceptionError::StaleRef { .. } => CmdError::new(ErrorCode::StaleRef, message)
            .with_hint("Take a new page_snapshot (or page_find) and use the refs from it."),
        PerceptionError::BadRef { .. } => CmdError::new(ErrorCode::NotFound, message)
            .with_hint("Refs look like @4:e3 and come from page_snapshot or page_find."),
        PerceptionError::NotFound(_) => CmdError::new(ErrorCode::NotFound, message),
        PerceptionError::BadFilter(_) => CmdError::invalid_params(message),
        PerceptionError::BadTree(_) => CmdError::internal(message),
    }
}

/// The data folder: the one the app put into the registry's extensions, else the default.
pub fn data_dir(ctx: &Ctx) -> DataDir {
    ctx.extensions
        .get::<DataDir>()
        .map(|data| (*data).clone())
        .or_else(|| DataDir::resolve(None).ok())
        .unwrap_or_else(|| DataDir::from_root("data"))
}

/// Milliseconds since the Unix epoch (for default file names).
pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// True when `path` is relative and stays inside its base folder by its text alone
/// (no `..`, no drive or root). Used for paths the caller gives relative to a data folder.
pub fn is_plain_relative(path: &Path) -> bool {
    path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Where to write an output file: `requested` (absolute, or relative to `folder`), or
/// `folder/<default_name>`. Creates the parent folder.
pub fn output_path(
    folder: &Path,
    requested: Option<&str>,
    default_name: String,
) -> Result<PathBuf, CmdError> {
    let path = match requested {
        None => folder.join(default_name),
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        // Re-joined from its parts, so "sub/x.pdf" gets the platform's separators.
        Some(p) if is_plain_relative(Path::new(p)) => {
            folder.join(Path::new(p).components().collect::<PathBuf>())
        }
        Some(p) => {
            return Err(CmdError::invalid_params(format!(
                "path '{p}' leaves {}: give an absolute path or a plain relative one",
                folder.display()
            )));
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CmdError::internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    Ok(path)
}

/// `std::fs::canonicalize` on Windows returns `\\?\C:\…` paths; Chromium and humans want
/// `C:\…`. Keeps `\\?\UNC\…` (network shares) as it is.
pub fn plain_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// Write `bytes` to `path`, with a clear error.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    std::fs::write(path, bytes)
        .map_err(|e| CmdError::internal(format!("cannot write {}: {e}", path.display())))
}
