//! Reader mode (`simplify.reader`): Mozilla Readability in an isolated world.
//!
//! Readability.js (Apache-2.0, vendored in `assets/` with its license header) runs in our own
//! isolated world (`Page.createIsolatedWorld` + `Runtime.evaluate {contextId}`), so the page's
//! scripts can neither see nor tamper with it, and no `Runtime.enable` is needed. It parses a
//! copy of the document; with `apply` the driver (`assets/reader.js`) swaps the page for a
//! clean, dark/light-aware version.

use bmcp_core::{CmdError, Ctx, ErrorCode, TabId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Mozilla Readability 0.6.0 (https://github.com/mozilla/readability), unmodified.
pub const READABILITY_JS: &str = include_str!("../assets/Readability.js");
/// Our driver: `(function (apply) { … })`.
const DRIVER_JS: &str = include_str!("../assets/reader.js");

/// Isolated world name for reader mode.
const WORLD: &str = "bmcp-reader";

/// What the driver returns (camelCase from JavaScript).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Article {
    title: String,
    byline: Option<String>,
    site_name: Option<String>,
    excerpt: Option<String>,
    lang: Option<String>,
    text: String,
    applied: bool,
}

/// The readable article of a page.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReaderOutput {
    pub tab: TabId,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Article text, cut to `max_chars`.
    pub text: String,
    /// Length of the full article text in characters.
    pub length: usize,
    /// True when `text` was cut.
    pub truncated: bool,
    /// True when the page was replaced by the clean version.
    pub applied: bool,
}

/// The JavaScript evaluated in the isolated world.
pub fn reader_expression(apply: bool) -> String {
    format!("{READABILITY_JS}\n;\n({DRIVER_JS})({apply})")
}

/// Run reader mode on `tab`.
pub async fn read(
    ctx: &Ctx,
    tab: TabId,
    apply: bool,
    max_chars: usize,
) -> Result<ReaderOutput, CmdError> {
    let call = |method: &'static str, params: Value| async move {
        ctx.backend
            .cdp(tab, method.to_string(), params)
            .await
            .map_err(CmdError::from)
    };
    let tree = call("Page.getFrameTree", json!({})).await?;
    let frame_id = tree["frameTree"]["frame"]["id"].clone();
    let world = call(
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "worldName": WORLD }),
    )
    .await?;
    let reply = call(
        "Runtime.evaluate",
        json!({
            "expression": reader_expression(apply),
            "contextId": world["executionContextId"],
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("JavaScript exception");
        return Err(CmdError::internal(format!("reader mode failed: {text}")));
    }
    let value = reply["result"]["value"].clone();
    if value.is_null() {
        return Err(CmdError::new(
            ErrorCode::NotFound,
            "no readable article found on this page",
        )
        .with_hint("Reader mode needs an article-like page; use page_read for other pages."));
    }
    let article: Article = serde_json::from_value(value)
        .map_err(|e| CmdError::internal(format!("unexpected reader result: {e}")))?;
    let length = article.text.chars().count();
    let (text, truncated) = truncate(&article.text, max_chars);
    Ok(ReaderOutput {
        tab,
        title: article.title,
        byline: article.byline,
        site_name: article.site_name,
        excerpt: article.excerpt,
        lang: article.lang,
        text,
        length,
        truncated,
        applied: article.applied,
    })
}

/// The first `max` characters (on a char boundary) and whether anything was cut.
fn truncate(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((cut, _)) => (format!("{}…", &text[..cut]), true),
        None => (text.to_string(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_readability_keeps_its_license_header() {
        assert!(READABILITY_JS.contains("Licensed under the Apache License, Version 2.0"));
        assert!(READABILITY_JS.contains("function Readability(doc, options)"));
        let expr = reader_expression(true);
        assert!(expr.ends_with("(true)"));
        assert!(expr.contains("new Readability(document.cloneNode(true)"));
    }

    #[test]
    fn truncate_counts_characters() {
        assert_eq!(truncate("héllo", 10), ("héllo".to_string(), false));
        assert_eq!(truncate("héllo", 2), ("hé…".to_string(), true));
    }
}
