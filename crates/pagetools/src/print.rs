//! Saving the page: PDF (`Page.printToPDF`) and MHTML (`Page.captureSnapshot`).
//!
//! Files go to `data/pdf/` and `data/mhtml/` unless a path is given (absolute, or relative
//! to that folder).

use std::path::PathBuf;

use base64::Engine;
use bmcp_core::{CmdError, Ctx, TabId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::util::{cdp, data_dir, now_ms, output_path, write_file};

/// Paper sizes for `page.pdf`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Paper {
    /// 8.5 × 11 in (Chromium's default).
    #[default]
    Letter,
    /// 8.5 × 14 in
    Legal,
    /// 11 × 17 in
    Tabloid,
    /// 297 × 420 mm
    A3,
    /// 210 × 297 mm
    A4,
    /// 148 × 210 mm
    A5,
}

impl Paper {
    /// Width and height in inches (what CDP wants), portrait.
    pub fn inches(self) -> (f64, f64) {
        const MM: f64 = 1.0 / 25.4;
        match self {
            Paper::Letter => (8.5, 11.0),
            Paper::Legal => (8.5, 14.0),
            Paper::Tabloid => (11.0, 17.0),
            Paper::A3 => (297.0 * MM, 420.0 * MM),
            Paper::A4 => (210.0 * MM, 297.0 * MM),
            Paper::A5 => (148.0 * MM, 210.0 * MM),
        }
    }
}

/// Print options (the arguments of `page.pdf` without tab and path).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfOptions {
    pub landscape: bool,
    pub print_background: bool,
    pub scale: f64,
    pub paper: Paper,
}

/// The `Page.printToPDF` parameters for `options`. `landscape` swaps the page sides itself.
pub fn pdf_params(options: &PdfOptions) -> Value {
    let (width, height) = options.paper.inches();
    json!({
        "landscape": options.landscape,
        "printBackground": options.print_background,
        "scale": options.scale,
        "paperWidth": width,
        "paperHeight": height,
        "transferMode": "ReturnAsBase64",
    })
}

/// Print `tab` to a PDF file; returns the path and the size in bytes.
pub async fn save_pdf(
    ctx: &Ctx,
    tab: TabId,
    path: Option<&str>,
    options: &PdfOptions,
) -> Result<(PathBuf, u64), CmdError> {
    if !(0.1..=2.0).contains(&options.scale) {
        return Err(CmdError::invalid_params("scale must be between 0.1 and 2"));
    }
    let reply = cdp(ctx, tab, "Page.printToPDF", pdf_params(options)).await?;
    let data = reply["data"]
        .as_str()
        .ok_or_else(|| CmdError::internal("the browser returned no PDF data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| CmdError::internal(format!("bad PDF data from the browser: {e}")))?;
    let folder = data_dir(ctx).root().join("pdf");
    let file = output_path(&folder, path, format!("tab{tab}-{}.pdf", now_ms()))?;
    write_file(&file, &bytes)?;
    Ok((file, bytes.len() as u64))
}

/// Save `tab` as one MHTML file (page + images + styles); returns the path and the size.
pub async fn save_mhtml(
    ctx: &Ctx,
    tab: TabId,
    path: Option<&str>,
) -> Result<(PathBuf, u64), CmdError> {
    let reply = cdp(
        ctx,
        tab,
        "Page.captureSnapshot",
        json!({ "format": "mhtml" }),
    )
    .await?;
    let data = reply["data"]
        .as_str()
        .ok_or_else(|| CmdError::internal("the browser returned no MHTML data"))?;
    let folder = data_dir(ctx).root().join("mhtml");
    let file = output_path(&folder, path, format!("tab{tab}-{}.mhtml", now_ms()))?;
    write_file(&file, data.as_bytes())?;
    Ok((file, data.len() as u64))
}
