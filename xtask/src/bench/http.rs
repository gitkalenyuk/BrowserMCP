//! Tiny HTTP pieces for the benchmark, std only (localhost, no TLS, `Connection: close`):
//!
//! * [`Fixtures`] — serves `tests/fixtures/sites/` and records `POST /bench/result` bodies
//!   (what the fixture pages report when the agent completes a step);
//! * [`request`] / [`rest`] — a client for the browser's REST API (`POST /v1/cmd/{name}`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::Result;

/// The fixture web server. Lives until the process ends.
pub struct Fixtures {
    pub port: u16,
    results: Arc<Mutex<Vec<Value>>>,
}

impl Fixtures {
    /// Serve `root` on 127.0.0.1 at a free port, in background threads.
    pub fn start(root: PathBuf) -> Result<Fixtures> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("fixture server: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let results = Arc::new(Mutex::new(Vec::new()));
        let shared = results.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (root, results) = (root.clone(), shared.clone());
                std::thread::spawn(move || {
                    if let Err(err) = handle(stream, &root, &results) {
                        eprintln!("fixture server: {err}");
                    }
                });
            }
        });
        Ok(Fixtures { port, results })
    }

    /// `http://127.0.0.1:<port>`
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Forget all recorded results (before each task).
    pub fn reset(&self) {
        self.results.lock().unwrap().clear();
    }

    /// Payloads reported for `task`, oldest first.
    pub fn results(&self, task: &str) -> Vec<Value> {
        self.results
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["task"] == task)
            .map(|r| r["payload"].clone())
            .collect()
    }
}

/// One request: `GET <file>` or `POST /bench/result`.
fn handle(stream: TcpStream, root: &Path, results: &Mutex<Vec<Value>>) -> Result {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let (method, path, body) = read_request(&mut reader)?;
    let mut stream = stream;
    let path = path.split(['?', '#']).next().unwrap_or("/").to_owned();

    if method == "POST" && path == "/bench/result" {
        let value: Value = serde_json::from_slice(&body).unwrap_or(json!({ "raw": true }));
        results.lock().unwrap().push(value);
        return respond(&mut stream, 200, "application/json", b"{\"ok\":true}");
    }
    if method != "GET" {
        return respond(&mut stream, 405, "text/plain", b"method not allowed");
    }
    // No `..`: only files below the fixture folder.
    let relative = path.trim_start_matches('/');
    if relative.split('/').any(|part| part == "..") {
        return respond(&mut stream, 403, "text/plain", b"forbidden");
    }
    let file = root.join(if relative.is_empty() {
        "index.html"
    } else {
        relative
    });
    match std::fs::read(&file) {
        Ok(bytes) => respond(&mut stream, 200, content_type(&file), &bytes),
        Err(_) => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Request line + headers + body (by `Content-Length`).
fn read_request(reader: &mut impl BufRead) -> Result<(String, String, Vec<u8>)> {
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or("/").to_owned();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok((method, path, body))
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        if status == 200 { "OK" } else { "Error" },
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|e| e.to_string())
}

/// One HTTP/1.1 request to 127.0.0.1:`port`; returns (status, body).
pub fn request(port: u16, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("cannot connect to 127.0.0.1:{port}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let body = body.unwrap_or("");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    parse_response(&raw)
}

/// Split a raw response into status and body (handles `Transfer-Encoding: chunked`).
pub fn parse_response(raw: &[u8]) -> Result<(u16, String)> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no HTTP status")?;
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_owned()
    };
    Ok((status, body))
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || rest.len() < size {
            break;
        }
        out.push_str(&rest[..size]);
        body = rest[size..].trim_start_matches("\r\n");
    }
    out
}

/// Call a browser command over REST: `POST /v1/cmd/{name}` with JSON `args`.
pub fn rest(port: u16, name: &str, args: Value) -> Result<Value> {
    let (status, body) = request(
        port,
        "POST",
        &format!("/v1/cmd/{name}"),
        Some(&args.to_string()),
    )?;
    let value: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));
    if status == 200 {
        Ok(value)
    } else {
        Err(format!("{name}: HTTP {status}: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_server_serves_files_and_records_results() {
        let dir = std::env::temp_dir().join(format!("bmcp-bench-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bench")).unwrap();
        std::fs::write(dir.join("bench/page.html"), "<h1>hi</h1>").unwrap();
        let fixtures = Fixtures::start(dir.clone()).unwrap();

        let (status, body) = request(fixtures.port, "GET", "/bench/page.html?x=1", None).unwrap();
        assert_eq!((status, body.as_str()), (200, "<h1>hi</h1>"));
        assert_eq!(
            request(fixtures.port, "GET", "/nope.html", None).unwrap().0,
            404
        );
        assert_eq!(
            request(fixtures.port, "GET", "/../secret", None).unwrap().0,
            403
        );

        let posted = r#"{"task":"t1","payload":{"answer":"42"}}"#;
        let (status, _) = request(fixtures.port, "POST", "/bench/result", Some(posted)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(fixtures.results("t1"), vec![json!({ "answer": "42" })]);
        assert!(fixtures.results("t2").is_empty());
        fixtures.reset();
        assert!(fixtures.results("t1").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chunked_responses_are_joined() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap(), (200, "{\"a\":1}".to_owned()));
    }
}
