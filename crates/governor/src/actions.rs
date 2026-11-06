//! The CDP sequences behind freezing, discarding and restoring one tab. No state here: the
//! [`crate::Governor`] decides *when*, these functions only know *how*.
//!
//! * freeze / thaw — `Page.setWebLifecycleState {state: "frozen" | "active"}`. A frozen page
//!   keeps its DOM and memory but runs no timers, tasks or animations (CPU → 0);
//! * discard — remember URL, title and scroll position (`Runtime.evaluate`), then navigate the
//!   tab to a tiny `data:` placeholder ("Discarded — click to restore"). The page's renderer
//!   memory is released; the tab keeps its id and its place in the tab strip;
//! * restore — navigate back to the remembered URL and scroll to the remembered position.
//!
//! **Back/forward cache.** Navigating away normally parks the old page in Chromium's bfcache
//! (up to 10 minutes), renderer and memory included — measured live: discarding 3 heavy tabs
//! freed nothing. `Page.resetNavigationHistory` does not evict it either. A page with an
//! `unload` handler is never cached, so discard adds an empty one right before it navigates
//! (measured: the renderer exits at once). Sites whose Permissions-Policy forbids `unload`
//! ignore it; they are freed when the bfcache entry expires.

use std::time::Duration;

use bmcp_core::{BackendError, BackendResult, BrowserBackend, TabId, TabInfo};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// CDP method that freezes and resumes a page.
pub const SET_LIFECYCLE: &str = "Page.setWebLifecycleState";
/// Small CDP calls (lifecycle, evaluate) must not hang the governor.
const CDP_TIMEOUT: Duration = Duration::from_secs(3);
/// The placeholder is a `data:` URL; it loads at once.
const PLACEHOLDER_TIMEOUT: Duration = Duration::from_secs(5);
/// Restoring loads the real page again.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(30);
/// Marker inside the placeholder page, so we recognise it by its URL.
const MARKER: &str = "bmcp-discarded";

/// What a discarded tab remembers to come back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Saved {
    pub url: String,
    pub title: String,
    pub scroll_x: f64,
    pub scroll_y: f64,
}

/// One CDP call with [`CDP_TIMEOUT`].
async fn cdp(
    backend: &dyn BrowserBackend,
    tab: TabId,
    method: &str,
    params: Value,
) -> BackendResult<Value> {
    tokio::time::timeout(CDP_TIMEOUT, backend.cdp(tab, method.into(), params))
        .await
        .map_err(|_| BackendError::Timeout(CDP_TIMEOUT))?
}

/// `Runtime.evaluate` of an expression whose result is JSON-serialisable; `None` on any error.
async fn eval(backend: &dyn BrowserBackend, tab: TabId, expression: &str) -> Option<Value> {
    let reply = cdp(
        backend,
        tab,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    )
    .await
    .ok()?;
    reply.get("result")?.get("value").cloned()
}

/// Freeze (`true`) or resume (`false`) the page.
pub async fn set_frozen(
    backend: &dyn BrowserBackend,
    tab: TabId,
    frozen: bool,
) -> BackendResult<()> {
    let state = if frozen { "frozen" } else { "active" };
    cdp(backend, tab, SET_LIFECYCLE, json!({ "state": state })).await?;
    Ok(())
}

/// Does the page play sound right now? Looks at `<audio>`/`<video>` elements of the top
/// document (Web Audio and media inside iframes are not seen). Errors count as "no".
pub async fn plays_audio(backend: &dyn BrowserBackend, tab: TabId) -> bool {
    const EXPR: &str = "[...document.querySelectorAll('audio,video')]\
        .some(m => !m.paused && !m.ended && !m.muted && m.volume > 0)";
    eval(backend, tab, EXPR).await == Some(Value::Bool(true))
}

/// The page's JS heap in bytes (`Runtime.getHeapUsage`), as a per-tab memory estimate.
pub async fn js_heap(backend: &dyn BrowserBackend, tab: TabId) -> Option<u64> {
    let reply = cdp(backend, tab, "Runtime.getHeapUsage", json!({}))
        .await
        .ok()?;
    reply.get("usedSize")?.as_f64().map(|n| n as u64)
}

/// Only real web pages are discarded; the new tab page, `about:` and `data:` pages are tiny.
pub fn is_discardable(url: &str) -> bool {
    ["http://", "https://", "file://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

/// True for the URL of our placeholder page.
pub fn is_placeholder(url: &str) -> bool {
    url.starts_with("data:text/html") && url.contains(MARKER)
}

/// The placeholder page as a `data:` URL. It keeps the page's title (so the tab strip still
/// shows it) and links to the page, so a click restores it even without the governor.
pub fn placeholder_url(saved: &Saved) -> String {
    let title = html_escape(&saved.title);
    let url = html_escape(&saved.url);
    let html = format!(
        "<!doctype html><!--{MARKER}--><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font:15px system-ui,sans-serif;color:#777;display:grid;place-items:center;\
         height:90vh;margin:0}}a{{color:#4a7bd0}}p{{font-size:12px;word-break:break-all}}</style>\
         <div><a href=\"{url}\">Discarded to save memory — click to restore</a><p>{url}</p></div>"
    );
    format!("data:text/html;charset=utf-8,{}", percent_encode(&html))
}

/// Discard: remember where we were, then load the placeholder. `frozen` = the page is frozen
/// now; it is resumed first (a frozen page cannot unload).
pub async fn discard(
    backend: &dyn BrowserBackend,
    info: &TabInfo,
    frozen: bool,
) -> BackendResult<Saved> {
    let tab = info.id;
    if frozen {
        set_frozen(backend, tab, false).await?;
    }
    // One call: keep the page out of the bfcache (see the module docs) and read the scroll.
    const EXPR: &str = "addEventListener('unload', () => {}); ({x: scrollX, y: scrollY})";
    let scroll = eval(backend, tab, EXPR).await;
    let coord = |key: &str| {
        scroll
            .as_ref()
            .and_then(|s| s.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let saved = Saved {
        url: info.url.clone(),
        title: info.title.clone(),
        scroll_x: coord("x"),
        scroll_y: coord("y"),
    };
    let placeholder = placeholder_url(&saved);
    match backend
        .navigate(tab, placeholder.clone(), PLACEHOLDER_TIMEOUT)
        .await
    {
        Ok(_) => Ok(saved),
        // Seen live: the placeholder shows but "loading finished" is not reported in time.
        Err(BackendError::Timeout(_)) if current_url(backend, tab).await == Some(placeholder) => {
            Ok(saved)
        }
        Err(err) => Err(err),
    }
}

/// Restore: load the remembered URL and scroll back. A slow page (timeout) still counts as
/// restored: it is loading the right URL.
pub async fn restore(backend: &dyn BrowserBackend, tab: TabId, saved: &Saved) -> BackendResult<()> {
    match backend
        .navigate(tab, saved.url.clone(), RESTORE_TIMEOUT)
        .await
    {
        Ok(_) | Err(BackendError::Timeout(_)) => {}
        Err(err) => return Err(err),
    }
    restore_scroll(backend, tab, saved).await;
    Ok(())
}

/// The tab's URL as the browser reports it now.
async fn current_url(backend: &dyn BrowserBackend, tab: TabId) -> Option<String> {
    let tabs = backend.list_tabs().await.ok()?;
    tabs.into_iter().find(|t| t.id == tab).map(|t| t.url)
}

/// Scroll to the remembered position. Pages that grow while loading (lazy images) may not be
/// tall enough yet, so it tries again a bit later inside the page.
pub async fn restore_scroll(backend: &dyn BrowserBackend, tab: TabId, saved: &Saved) {
    if saved.scroll_x == 0.0 && saved.scroll_y == 0.0 {
        return;
    }
    let (x, y) = (saved.scroll_x, saved.scroll_y);
    let expr = format!(
        "(() => {{ const go = () => scrollTo({x}, {y}); go(); \
         setTimeout(go, 300); setTimeout(go, 1000); return true; }})()"
    );
    let _ = eval(backend, tab, &expr).await;
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Percent-encode everything except unreserved URL characters (RFC 3986).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_recognised_and_escaped() {
        let saved = Saved {
            url: "https://a.test/?q=<x>&y=\"1\"".into(),
            title: "A & B".into(),
            scroll_x: 0.0,
            scroll_y: 10.0,
        };
        let url = placeholder_url(&saved);
        assert!(is_placeholder(&url));
        assert!(!is_placeholder("https://a.test/"));
        assert!(!is_placeholder("data:text/html,hello"));
        // Nothing but unreserved characters and %XX after the comma.
        let body = url.split_once(',').unwrap().1;
        assert!(
            body.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~%".contains(&b))
        );
        assert!(body.contains("A%20%26amp%3B%20B"));
        assert!(body.contains("%26lt%3Bx%26gt%3B"));
    }

    #[test]
    fn only_web_pages_are_discardable() {
        assert!(is_discardable("https://a.test/"));
        assert!(is_discardable("http://127.0.0.1:8000/x"));
        assert!(is_discardable("file:///C:/x.html"));
        assert!(!is_discardable("bmcp://ui/newtab.html"));
        assert!(!is_discardable("about:blank"));
        assert!(!is_discardable("data:text/html,x"));
    }
}
