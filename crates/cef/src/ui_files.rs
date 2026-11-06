//! Files served under `bmcp://ui/`: the Svelte build (`ui/dist`).
//!
//! * Release and normal debug builds embed `ui/dist` into the executable at compile time
//!   (`build.rs` + `include_dir!`). If `ui/dist` did not exist when Rust was built, a
//!   one-page "UI not built" placeholder is embedded instead (`assets/fallback/`).
//! * `--ui-dir <path>` serves the files from a folder on disk at run time instead
//!   (rebuild the UI with `pnpm -C ui build`, reload the page, no Rust rebuild).
//!
//! Every HTML page gets a tiny config script injected right after `<head>`:
//! `window.__BMCP__ = {"ws": "ws://127.0.0.1:<port>/v1/ws"}`, so the UI (and the new tab
//! page, whose URL must stay a clean `bmcp://ui/newtab.html`) knows where the API is.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};

/// `ui/dist` (or the placeholder), chosen by `build.rs`.
static EMBEDDED: Dir<'static> = include_dir!("$BMCP_UI_DIST");

/// Where `bmcp://ui/` files come from.
#[derive(Debug, Clone, Default)]
pub enum Source {
    #[default]
    Embedded,
    Folder(PathBuf),
}

/// One file ready to be served.
pub struct File {
    pub mime: &'static str,
    pub body: Vec<u8>,
}

/// Look up `bmcp://ui/<path>` (path without query/fragment; empty = `index.html`).
/// `config_json` is injected into HTML pages. `None` = not found (or a path that tries to
/// leave the folder).
pub fn load(source: &Source, path: &str, config_json: &str) -> Option<File> {
    let path = clean_path(path)?;
    let bytes: Cow<'static, [u8]> = match source {
        Source::Embedded => Cow::Borrowed(EMBEDDED.get_file(path.as_ref())?.contents()),
        Source::Folder(root) => Cow::Owned(std::fs::read(root.join(path.as_ref())).ok()?),
    };
    let mime = mime_for(&path);
    let body = if mime == "text/html" {
        inject_config(&bytes, config_json)
    } else {
        bytes.into_owned()
    };
    Some(File { mime, body })
}

/// Normalize a request path: empty → `index.html`; reject `..`, backslashes, absolute
/// paths and empty segments so a request can never leave the UI folder.
pub fn clean_path(path: &str) -> Option<Cow<'_, str>> {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Some(Cow::Borrowed("index.html"));
    }
    let bad = path.contains('\\')
        || path.contains(':')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..");
    if bad {
        return None;
    }
    Some(Cow::Borrowed(path))
}

/// MIME type by file extension (only what a Vite build produces).
pub fn mime_for(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        // Monaco's codicon font (Scripts panel).
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Insert `<script>window.__BMCP__=…</script>` right after `<head>` (or at the very start
/// if there is no `<head>`). `</` is escaped so the JSON cannot end the script early.
pub fn inject_config(html: &[u8], config_json: &str) -> Vec<u8> {
    let script = format!("<script>{}</script>", config_script(config_json));
    let at = find_head_end(html).unwrap_or(0);
    let mut out = Vec::with_capacity(html.len() + script.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(script.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
}

/// The text inside the injected `<script>` (its SHA-256 is allowed by the pages' CSP, see
/// `scheme::content_security_policy`).
pub fn config_script(config_json: &str) -> String {
    format!("window.__BMCP__={};", config_json.replace("</", "<\\/"))
}

/// Byte offset just after the opening `<head …>` tag (ASCII case-insensitive).
fn find_head_end(html: &[u8]) -> Option<usize> {
    let lower: Vec<u8> = html.iter().map(u8::to_ascii_lowercase).collect();
    let start = lower.windows(5).position(|w| w == b"<head")?;
    let close = lower[start..].iter().position(|b| *b == b'>')?;
    Some(start + close + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_cannot_escape() {
        assert_eq!(clean_path("").as_deref(), Some("index.html"));
        assert_eq!(clean_path("/").as_deref(), Some("index.html"));
        assert_eq!(
            clean_path("assets/app-1.js").as_deref(),
            Some("assets/app-1.js")
        );
        assert_eq!(clean_path("../secret"), None);
        assert_eq!(clean_path("a/../../b"), None);
        assert_eq!(clean_path("a//b"), None);
        assert_eq!(clean_path("a\\b"), None);
        assert_eq!(clean_path("C:/Windows/win.ini"), None);
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for("index.html"), "text/html");
        assert_eq!(mime_for("assets/x.JS"), "text/javascript");
        assert_eq!(mime_for("assets/x.css"), "text/css");
        assert_eq!(mime_for("x.svg"), "image/svg+xml");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }

    #[test]
    fn config_goes_after_head() {
        let out = inject_config(
            b"<!doctype html><html><HEAD lang=en><title>t</title></head></html>",
            r#"{"ws":"ws://x/</script>"}"#,
        );
        let out = String::from_utf8(out).unwrap();
        assert_eq!(
            out,
            "<!doctype html><html><HEAD lang=en><script>window.__BMCP__={\"ws\":\"ws://x/<\\/script>\"};</script><title>t</title></head></html>"
        );
        let out = inject_config(b"<p>no head</p>", "{}");
        assert!(out.starts_with(b"<script>window.__BMCP__={};</script><p>"));
    }

    #[test]
    fn embedded_ui_has_the_entry_pages() {
        // Either the real build or the placeholder: both provide these entries.
        for page in ["index.html", "palette.html", "newtab.html"] {
            let file = load(&Source::Embedded, page, "{}")
                .unwrap_or_else(|| panic!("{page} missing from the embedded UI"));
            assert_eq!(file.mime, "text/html");
            assert!(String::from_utf8_lossy(&file.body).contains("window.__BMCP__={}"));
        }
        assert!(load(&Source::Embedded, "nope.html", "{}").is_none());
    }

    #[test]
    fn folder_source_reads_from_disk() {
        let dir = std::env::temp_dir().join(format!("bmcp-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/a.css"), "body{}").unwrap();
        let file = load(&Source::Folder(dir.clone()), "assets/a.css", "{}").unwrap();
        assert_eq!(file.mime, "text/css");
        assert_eq!(file.body, b"body{}");
        assert!(load(&Source::Folder(dir.clone()), "../x", "{}").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
