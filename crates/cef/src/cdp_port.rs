//! Chromium's remote debugging port (`--cdp-port`, `Settings::remote_debugging_port`).
//!
//! Pitfall (CEF 156, found with `--headless --cdp-port`): with a remote debugging port and
//! no startup window, Chrome's DevTools manager holds a keep-alive (origin
//! `REMOTE_DEBUGGING`) "until an explicit close through the DevTools protocol". So
//! `quit_message_loop()` alone never ended the message loop — the process hung after
//! `system.quit` and after the last window closed. [`release_keep_alive`] sends that explicit
//! close (`Browser.close` on the browser target) right after `quit_message_loop()`; the loop
//! then ends and `cef::shutdown()` runs as usual.
//!
//! The browser target is reached over the port itself: `GET /json/version` names its
//! WebSocket, and one masked text frame carries the command. A hand-rolled minimal client
//! (std only, loopback only) — nothing else here needs a WebSocket client.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::OnceLock;
use std::time::Duration;

static PORT: OnceLock<u16> = OnceLock::new();

/// The port CEF was started with. Before CEF initializes.
pub(crate) fn configure(port: u16) {
    let _ = PORT.set(port);
}

/// Let the process end: send `Browser.close` over the debugging port, in the background.
/// No-op without a port. Call after `quit_message_loop()` (UI thread; does not block).
pub(crate) fn release_keep_alive() {
    let Some(port) = PORT.get().copied() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("bmcp-cdp-close".into())
        .spawn(move || match send_browser_close(port) {
            Ok(()) => tracing::debug!(port, "Browser.close sent over the debugging port"),
            Err(err) => tracing::warn!(port, %err, "could not send Browser.close; quit may hang"),
        });
    if let Err(err) = spawned {
        tracing::warn!(%err, "could not start the Browser.close thread");
    }
}

const TIMEOUT: Duration = Duration::from_secs(5);

fn connect(port: u16) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    Ok(stream)
}

fn other(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

fn send_browser_close(port: u16) -> std::io::Result<()> {
    // 1. Which WebSocket is the browser target?
    let mut stream = connect(port)?;
    write!(
        stream,
        "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )?;
    // Pitfall: the DevTools server keeps the connection open despite `Connection: close`,
    // so read exactly Content-Length bytes instead of reading to the end.
    let head = read_head(&mut stream)?;
    let length = content_length(&head).ok_or_else(|| other("no Content-Length"))?;
    let mut body = vec![0u8; length.min(64 * 1024)];
    stream.read_exact(&mut body)?;
    let answer = String::from_utf8_lossy(&body);
    let path = browser_ws_path(&answer)
        .ok_or_else(|| other("no webSocketDebuggerUrl in /json/version"))?;

    // 2. Upgrade, then one text frame with the command.
    let mut stream = connect(port)?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    )?;
    let head = read_head(&mut stream)?;
    if !head.starts_with("HTTP/1.1 101") {
        return Err(other(format!(
            "upgrade refused: {}",
            head.lines().next().unwrap_or("")
        )));
    }
    stream.write_all(&text_frame(br#"{"id":1,"method":"Browser.close"}"#))?;
    stream.flush()?;
    // The answer (or the connection closing as the browser goes away) — either is fine.
    let mut buf = [0u8; 256];
    let _ = stream.read(&mut buf);
    Ok(())
}

/// The response head up to the blank line.
fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 || head.len() > 8192 {
            break;
        }
        head.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())?
    })
}

/// `/devtools/browser/<id>` from the `/json/version` body.
fn browser_ws_path(answer: &str) -> Option<String> {
    let at = answer.find("\"webSocketDebuggerUrl\"")?;
    let rest = &answer[at..];
    let url_start = rest.find("ws://")? + "ws://".len();
    let url = &rest[url_start..];
    let url = &url[..url.find('"')?];
    let path = &url[url.find('/')?..];
    Some(path.to_owned())
}

/// A final, masked (client → server) text frame. Payloads here are < 126 bytes.
fn text_frame(payload: &[u8]) -> Vec<u8> {
    debug_assert!(payload.len() < 126);
    let mask = [0x12, 0x34, 0x56, 0x78];
    let mut frame = vec![0x81, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_browser_target() {
        let answer = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\n   \"Browser\": \
                      \"Chrome/156.0\",\n   \"webSocketDebuggerUrl\": \
                      \"ws://127.0.0.1:9333/devtools/browser/0ea8-33fe\"\n}\n";
        assert_eq!(
            browser_ws_path(answer).as_deref(),
            Some("/devtools/browser/0ea8-33fe")
        );
        assert_eq!(browser_ws_path("HTTP/1.1 404 Not Found\r\n\r\n"), None);
    }

    #[test]
    fn reads_content_length() {
        let head = "HTTP/1.1 200 OK\r\ncontent-length: 42\r\nContent-Type: x\r\n\r\n";
        assert_eq!(content_length(head), Some(42));
        assert_eq!(content_length("HTTP/1.1 200 OK\r\n\r\n"), None);
    }

    #[test]
    fn frames_are_masked() {
        let frame = text_frame(b"hi");
        assert_eq!(&frame[..2], &[0x81, 0x82]);
        let mask = &frame[2..6];
        let unmasked: Vec<u8> = frame[6..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        assert_eq!(unmasked, b"hi");
    }
}
