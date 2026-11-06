//! Compact text for agents: one line per request, short sizes and URLs.
//!
//! `#12 GET 200 xhr 3.4kB 120ms https://api.test/items?page=2`

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use crate::store::{Body, Entry, cut};

/// Longest URL printed in a list line.
pub const LIST_URL_CHARS: usize = 120;

/// `512B`, `3.4kB`, `1.2MB` (1000-based, like Chrome DevTools).
pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        format!("{bytes}B")
    } else if bytes < 1_000_000 {
        format!("{:.1}kB", bytes as f64 / 1000.0)
    } else {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    }
}

/// `120ms`, `3.2s`.
pub fn duration(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{}ms", ms.round() as u64)
    } else {
        format!("{:.1}s", ms / 1000.0)
    }
}

/// Cut a long URL in the middle: `https://api.test/v1/very…/items?page=2`.
pub fn short_url(url: &str, max_chars: usize) -> String {
    let count = url.chars().count();
    if count <= max_chars {
        return url.to_string();
    }
    let tail = max_chars * 2 / 5;
    let head = max_chars.saturating_sub(tail + 1);
    let start: String = url.chars().take(head).collect();
    let end: String = url.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

/// One list line for an entry.
pub fn line(e: &Entry) -> String {
    let status = match (&e.error, e.status) {
        (Some(_), _) => "ERR".to_string(),
        (None, Some(status)) => status.to_string(),
        (None, None) => "…".to_string(),
    };
    let size = if e.from_cache {
        "cache".to_string()
    } else if e.size > 0 {
        size(e.size)
    } else {
        "-".to_string()
    };
    let time = match e.duration_ms() {
        Some(ms) => duration(ms),
        None if e.resource_type == "websocket" && e.error.is_none() => "open".into(),
        None => "-".into(),
    };
    let mut out = format!(
        "#{} {} {} {} {} {} {}",
        e.id,
        e.method,
        status,
        e.resource_type,
        size,
        time,
        short_url(&e.url, LIST_URL_CHARS)
    );
    if let Some(err) = &e.error {
        out.push_str(&format!(" !{err}"));
    }
    if e.resource_type == "websocket" && !e.frames.is_empty() {
        out.push_str(&format!(
            " ({} frames)",
            e.frames.len() as u64 + e.frames_dropped
        ));
    }
    if !e.notes.is_empty() {
        out.push_str(&format!(" [{}]", e.notes.join("; ")));
    }
    out
}

/// A body as text for output, cut to `max_bytes`. Binary bodies are described, not dumped.
/// Returns the text and whether it was cut.
pub fn body_text(body: &Body, mime: &str, max_bytes: usize) -> (String, bool) {
    let text = if body.base64 {
        let bytes = B64.decode(&body.data).unwrap_or_default();
        match String::from_utf8(bytes) {
            Ok(text) if is_textual(mime) || !text.contains('\0') => text,
            _ => {
                return (
                    format!(
                        "[binary {} {}, not shown]",
                        size(body.full_len as u64),
                        mime
                    ),
                    false,
                );
            }
        }
    } else {
        body.data.clone()
    };
    let kept = cut(&text, max_bytes);
    let truncated = kept.len() < text.len() || body.truncated;
    (kept.to_string(), truncated)
}

/// MIME types whose bodies are text.
pub fn is_textual(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.starts_with("text/")
        || mime.contains("json")
        || mime.contains("javascript")
        || mime.contains("xml")
        || mime.contains("x-www-form-urlencoded")
        || mime.contains("graphql")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_durations() {
        assert_eq!(size(512), "512B");
        assert_eq!(size(3400), "3.4kB");
        assert_eq!(size(1_250_000), "1.2MB");
        assert_eq!(duration(119.6), "120ms");
        assert_eq!(duration(3210.0), "3.2s");
    }

    #[test]
    fn long_urls_are_cut_in_the_middle() {
        let url = format!("https://api.test/{}/items?page=2", "x".repeat(200));
        let short = short_url(&url, 60);
        assert_eq!(short.chars().count(), 60);
        assert!(short.starts_with("https://api.test/"));
        assert!(short.ends_with("/items?page=2"));
        assert_eq!(short_url("https://a.test/", 60), "https://a.test/");
    }
}
