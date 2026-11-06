//! A tiny Server-Sent Events parser (enough for model APIs).
//!
//! Bytes arrive in arbitrary chunks (a UTF-8 character or a line may be split), so the parser
//! buffers until it sees a blank line, which ends one event:
//!
//! ```text
//! event: content_block_delta
//! data: {"type":"content_block_delta", ...}
//!
//! ```

use futures_util::StreamExt;

use super::ModelError;

/// One parsed event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// The `event:` field; empty when absent.
    pub event: String,
    /// All `data:` lines joined with `\n`.
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    /// Feed bytes; returns the events completed by them.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        // `\r` is only a line-ending helper in SSE; dropping it makes `\r\n` behave like `\n`.
        self.buf
            .extend(chunk.iter().copied().filter(|&b| b != b'\r'));
        let mut events = Vec::new();
        while let Some(end) = find_blank_line(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end + 2).collect();
            if let Some(event) = parse_block(&String::from_utf8_lossy(&block)) {
                events.push(event);
            }
        }
        events
    }

    /// Whatever is left when the stream ends without a final blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let rest = std::mem::take(&mut self.buf);
        parse_block(&String::from_utf8_lossy(&rest))
    }
}

fn find_blank_line(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

fn parse_block(block: &str) -> Option<SseEvent> {
    let mut event = SseEvent::default();
    let mut data_lines = Vec::new();
    for line in block.lines() {
        // Lines starting with ':' are comments (keep-alives).
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event.event = value.to_string(),
            "data" => data_lines.push(value),
            _ => {}
        }
    }
    if data_lines.is_empty() && event.event.is_empty() {
        return None;
    }
    event.data = data_lines.join("\n");
    Some(event)
}

/// Read a whole SSE response, calling `on_event` for every event in order.
/// `on_event` returns `Ok(false)` to stop early (e.g. on OpenAI's `data: [DONE]`).
pub async fn for_each_event(
    resp: reqwest::Response,
    mut on_event: impl FnMut(SseEvent) -> Result<bool, ModelError>,
) -> Result<(), ModelError> {
    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        for event in parser.push(&chunk?) {
            if !on_event(event)? {
                return Ok(());
            }
        }
    }
    if let Some(event) = parser.finish() {
        on_event(event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_across_chunks_and_crlf() {
        let mut p = SseParser::default();
        assert!(p.push(b"event: a\r\ndata: {\"x\"").is_empty());
        let events = p.push(b":1}\r\n\r\n: keep-alive\n\ndata: second\ndata: line\n\n");
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: "a".into(),
                    data: "{\"x\":1}".into()
                },
                SseEvent {
                    event: "".into(),
                    data: "second\nline".into()
                },
            ]
        );
        assert_eq!(p.push(b"data: tail").len(), 0);
        assert_eq!(p.finish().unwrap().data, "tail");
    }

    #[test]
    fn utf8_split_inside_a_character() {
        let mut p = SseParser::default();
        let text = "data: привіт\n\n".as_bytes();
        let (a, b) = text.split_at(8); // in the middle of "п"
        assert!(p.push(a).is_empty());
        assert_eq!(p.push(b)[0].data, "привіт");
    }
}
