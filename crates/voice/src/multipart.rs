//! A small `multipart/form-data` body builder (text fields + one file), the format of every
//! `/audio/transcriptions`-style API.

use std::sync::atomic::{AtomicU64, Ordering};

/// A finished body and its `Content-Type` header value.
pub struct Multipart {
    pub content_type: String,
    pub body: Vec<u8>,
}

pub struct Builder {
    boundary: String,
    body: Vec<u8>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Builder {
            boundary: format!("----bmcp-voice-{nanos:x}-{n:x}"),
            body: Vec::new(),
        }
    }

    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    /// A text field (quotes and line breaks in the name are not allowed: names are ours).
    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// A text field when `value` is `Some` and not blank.
    pub fn text_opt(self, name: &str, value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => self.text(name, v),
            None => self,
        }
    }

    pub fn file(mut self, name: &str, filename: &str, mime: &str, bytes: &[u8]) -> Self {
        let filename: String = filename
            .chars()
            .filter(|c| !matches!(c, '"' | '\r' | '\n'))
            .collect();
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn finish(mut self) -> Multipart {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        Multipart {
            content_type: format!("multipart/form-data; boundary={}", self.boundary),
            body: self.body,
        }
    }
}

/// Parse a body made by [`Builder`] back into `(name, filename, value)` parts (tests and the
/// mock servers in them).
pub fn parse(content_type: &str, body: &[u8]) -> Vec<(String, Option<String>, Vec<u8>)> {
    let Some(boundary) = content_type.split("boundary=").nth(1) else {
        return Vec::new();
    };
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(start) = find(rest, delim.as_bytes()) {
        rest = &rest[start + delim.len()..];
        if rest.starts_with(b"--") {
            break;
        }
        let rest_after_crlf = rest.strip_prefix(b"\r\n").unwrap_or(rest);
        let Some(head_end) = find(rest_after_crlf, b"\r\n\r\n") else {
            break;
        };
        let head = String::from_utf8_lossy(&rest_after_crlf[..head_end]).to_string();
        let content = &rest_after_crlf[head_end + 4..];
        let Some(end) = find(content, format!("\r\n{delim}").as_bytes()) else {
            break;
        };
        let attr = |key: &str| {
            head.split(';')
                .map(str::trim)
                .find_map(|kv| kv.strip_prefix(&format!("{key}=\"")))
                .map(|v| {
                    v.trim_end_matches('"')
                        .split("\"")
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
        };
        parts.push((
            attr("name").unwrap_or_default(),
            attr("filename"),
            content[..end].to_vec(),
        ));
        rest = &content[end + 2..];
    }
    parts
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_fields_and_a_file_that_parse_back() {
        let m = Builder::new()
            .text("model", "whisper-1")
            .text_opt("language", Some(" uk "))
            .text_opt("prompt", Some("  "))
            .text_opt("x", None)
            .file("file", "a\"b.wav", "audio/wav", b"RIFF\r\n--xx")
            .finish();
        assert!(
            m.content_type
                .starts_with("multipart/form-data; boundary=----bmcp-voice-")
        );
        let parts = parse(&m.content_type, &m.body);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], ("model".into(), None, b"whisper-1".to_vec()));
        assert_eq!(parts[1], ("language".into(), None, b"uk".to_vec()));
        assert_eq!(
            parts[2],
            (
                "file".into(),
                Some("ab.wav".into()),
                b"RIFF\r\n--xx".to_vec()
            )
        );
        assert!(m.body.ends_with(b"--\r\n"));
    }
}
