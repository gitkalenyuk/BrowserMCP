//! File upload: `<input type=file>` directly, or the file chooser a button opens.
//!
//! * The ref is a file input → `DOM.setFileInputFiles` on it (fires `input` + `change`).
//! * Anything else (a "Choose photo" button, a label, a drop zone that calls
//!   `input.click()`) → `Page.setInterceptFileChooserDialog {enabled:true}`, a real click on
//!   the element (`Input.dispatchMouseEvent`, so the page sees a user gesture), wait for
//!   `Page.fileChooserOpened {backendNodeId, mode}` and fill that input. No native dialog is
//!   shown while interception is on; it is switched off again afterwards.
//!
//! Paths: absolute, or relative to a run folder `data/runs/<run_id>/` (the files a mass run
//! downloaded or was given). Every file must exist; a relative path may not leave the run
//! folder (`..`, links).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bmcp_core::{BrowserBackend, CmdError, Ctx, ErrorCode, TabId};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::util::{cdp, plain_path};

/// How long we wait for the chooser after clicking, unless the caller says otherwise.
pub const DEFAULT_CHOOSER_TIMEOUT: Duration = Duration::from_secs(5);

/// How the files got into the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The ref was the `<input type=file>` itself.
    Input,
    /// Clicking the ref opened a file chooser, which we filled.
    Chooser,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Input => "input",
            Via::Chooser => "chooser",
        }
    }
}

/// Check and resolve the paths: absolute ones as they are, relative ones inside
/// `data_root/runs/<run_id>/`. Returns plain absolute paths (`C:\…`, not `\\?\C:\…`).
pub fn resolve_files(
    data_root: &Path,
    run_id: Option<&str>,
    files: &[String],
) -> Result<Vec<PathBuf>, CmdError> {
    if files.is_empty() {
        return Err(CmdError::invalid_params(
            "files is empty: give at least one path",
        ));
    }
    let run_dir = match run_id {
        None => None,
        Some(id) => Some(run_folder(data_root, id)?),
    };
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let given = Path::new(file);
        let full = if given.is_absolute() {
            given.to_path_buf()
        } else {
            match &run_dir {
                Some(dir) => dir.join(given),
                None => {
                    return Err(CmdError::invalid_params(format!(
                        "'{file}' is relative: give an absolute path, or run_id to resolve it \
                         inside data/runs/<run_id>/"
                    )));
                }
            }
        };
        // `canonicalize` follows `..` and links, so the escape check below sees the truth.
        let real = std::fs::canonicalize(&full).map_err(|_| {
            CmdError::new(ErrorCode::NotFound, format!("file not found: {file}"))
                .with_retry(bmcp_core::Retry::No)
        })?;
        if let Some(dir) = &run_dir
            && !given.is_absolute()
            && !real.starts_with(dir)
        {
            return Err(CmdError::invalid_params(format!(
                "'{file}' leaves the run folder data/runs/{}/",
                run_id.unwrap_or_default()
            )));
        }
        if !real.is_file() {
            return Err(CmdError::invalid_params(format!("'{file}' is not a file")));
        }
        out.push(plain_path(real));
    }
    Ok(out)
}

/// `data_root/runs/<id>` (canonical), which must exist. The id is a plain folder name.
fn run_folder(data_root: &Path, id: &str) -> Result<PathBuf, CmdError> {
    let plain_name = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !plain_name {
        return Err(CmdError::invalid_params(format!(
            "bad run_id '{id}': use the id from run_list"
        )));
    }
    let dir = data_root.join("runs").join(id);
    std::fs::canonicalize(&dir)
        .ok()
        .filter(|d| d.is_dir())
        .ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotFound,
                format!("run folder data/runs/{id}/ does not exist"),
            )
            .with_retry(bmcp_core::Retry::No)
            .with_hint("Use a run id from run_list (its folder is created by the run).")
        })
}

/// Put `files` into the page through the element `backend_node_id` (see the module docs).
pub async fn upload(
    ctx: &Ctx,
    tab: TabId,
    backend_node_id: i64,
    files: &[String],
    timeout: Duration,
) -> Result<Via, CmdError> {
    let node = cdp(
        ctx,
        tab,
        "DOM.describeNode",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await?;
    if let Some(multiple) = file_input(&node["node"]) {
        check_count(files.len(), multiple)?;
        set_files(ctx, tab, backend_node_id, files).await?;
        return Ok(Via::Input);
    }

    // Not an input: click it and fill the chooser it opens. Listen before clicking.
    let mut events = ctx.backend.cdp_events(tab).await?;
    cdp(ctx, tab, "Page.enable", json!({})).await?;
    cdp(
        ctx,
        tab,
        "Page.setInterceptFileChooserDialog",
        json!({ "enabled": true }),
    )
    .await?;
    // Switches interception off again when this function ends in any way.
    let _off = InterceptOff {
        backend: ctx.backend.clone(),
        tab,
    };
    let result = async {
        click(ctx, tab, backend_node_id).await?;
        let opened = tokio::time::timeout(timeout, async {
            loop {
                match events.recv().await {
                    Ok(e) if e.method == "Page.fileChooserOpened" && e.session_id.is_none() => {
                        return Some(e.params);
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return None,
                }
            }
        })
        .await;
        let params = match opened {
            Ok(Some(params)) => params,
            Ok(None) => return Err(CmdError::new(ErrorCode::TabClosed, "the tab closed")),
            Err(_) => {
                return Err(CmdError::new(
                    ErrorCode::NotInteractable,
                    format!(
                        "clicking the element did not open a file chooser within {} ms",
                        timeout.as_millis()
                    ),
                )
                .with_hint(
                    "Give the ref of the <input type=file> itself, or of the button that \
                     opens the chooser (take a page_snapshot).",
                ));
            }
        };
        check_count(files.len(), params["mode"] == "selectMultiple")?;
        let input = params["backendNodeId"]
            .as_i64()
            .ok_or_else(|| CmdError::internal("file chooser without an input element"))?;
        set_files(ctx, tab, input, files).await
    }
    .await;
    result.map(|()| Via::Chooser)
}

/// Switches file-chooser interception off when dropped: after success, after an error, and
/// also when the command is cancelled mid-way (e.g. `DIALOG_OPEN` because the click opened
/// an alert). Later choosers belong to the human again.
struct InterceptOff {
    backend: Arc<dyn BrowserBackend>,
    tab: TabId,
}

impl Drop for InterceptOff {
    fn drop(&mut self) {
        let (backend, tab) = (self.backend.clone(), self.tab);
        // `drop` cannot await, so the CDP call goes out from a task.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let off = json!({ "enabled": false });
                let _ = backend
                    .cdp(tab, "Page.setInterceptFileChooserDialog".into(), off)
                    .await;
            });
        }
    }
}

/// `Some(multiple)` when the node is an `<input type=file>`.
fn file_input(node: &Value) -> Option<bool> {
    if !node["nodeName"]
        .as_str()
        .is_some_and(|n| n.eq_ignore_ascii_case("input"))
    {
        return None;
    }
    // `attributes` is a flat list: [name1, value1, name2, value2, …].
    let attrs: Vec<&str> = node["attributes"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let attr = |name: &str| {
        attrs
            .chunks(2)
            .find(|pair| pair[0].eq_ignore_ascii_case(name))
            .map(|pair| pair.get(1).copied().unwrap_or_default())
    };
    let is_file = attr("type").is_some_and(|t| t.eq_ignore_ascii_case("file"));
    is_file.then(|| attr("multiple").is_some())
}

fn check_count(count: usize, multiple: bool) -> Result<(), CmdError> {
    if count > 1 && !multiple {
        return Err(CmdError::invalid_params(format!(
            "this file input takes one file, got {count}"
        )));
    }
    Ok(())
}

async fn set_files(
    ctx: &Ctx,
    tab: TabId,
    backend_node_id: i64,
    files: &[String],
) -> Result<(), CmdError> {
    cdp(
        ctx,
        tab,
        "DOM.setFileInputFiles",
        json!({ "files": files, "backendNodeId": backend_node_id }),
    )
    .await?;
    Ok(())
}

/// A real left click in the middle of the element (scrolled into view first).
async fn click(ctx: &Ctx, tab: TabId, backend_node_id: i64) -> Result<(), CmdError> {
    let _ = cdp(
        ctx,
        tab,
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await;
    let quads = cdp(
        ctx,
        tab,
        "DOM.getContentQuads",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await
    .ok();
    // A quad is 4 corners: [x1, y1, x2, y2, x3, y3, x4, y4] in viewport CSS pixels.
    let corners: Vec<f64> = quads
        .as_ref()
        .and_then(|q| q["quads"][0].as_array())
        .map(|q| q.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    if corners.len() != 8 {
        return Err(CmdError::new(
            ErrorCode::NotInteractable,
            "the element is not visible (it has no box on the page)",
        )
        .with_hint("Take a page_snapshot; it may be hidden or collapsed."));
    }
    let x = (corners[0] + corners[2] + corners[4] + corners[6]) / 4.0;
    let y = (corners[1] + corners[3] + corners[5] + corners[7]) / 4.0;
    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
        let mut event = json!({ "type": kind, "x": x, "y": y });
        if kind != "mouseMoved" {
            event["button"] = json!("left");
            event["clickCount"] = json!(1);
        }
        cdp(ctx, tab, "Input.dispatchMouseEvent", event).await?;
    }
    Ok(())
}
