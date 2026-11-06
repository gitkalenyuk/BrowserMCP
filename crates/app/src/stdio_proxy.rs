//! `browsermcp mcp-stdio` — MCP over stdio for clients that cannot speak HTTP.
//!
//! It never starts CEF itself (CEF allows one process per profile folder). Instead it:
//! 1. finds the running browser through `data/runtime/endpoint.json`, starting the browser
//!    in the background if nothing answers;
//! 2. forwards every JSON-RPC line from stdin to `POST /mcp` and writes every JSON-RPC
//!    message from the answer (plain JSON or an SSE stream) to stdout, one per line.
//!
//! stdout carries protocol only; diagnostics go to stderr.

use std::io::{BufRead, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use bmcp_core::DataDir;
use serde_json::{Value, json};

pub fn run(data: &DataDir, api_port: u16) -> anyhow::Result<()> {
    let url = ensure_browser(data, api_port)?;
    eprintln!("browsermcp mcp-stdio: forwarding to {url}");

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into();
    let session: Arc<Mutex<Option<String>>> = Arc::default();
    let stdout = Arc::new(Mutex::new(std::io::stdout()));

    for line in std::io::stdin().lock().lines() {
        let line = line.context("reading stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        let is_initialize = line.contains("\"initialize\"");
        let (agent, url, session, stdout) =
            (agent.clone(), url.clone(), session.clone(), stdout.clone());
        let forward = move || {
            for message in forward_one(&agent, &url, &session, &line) {
                let mut out = stdout.lock().unwrap_or_else(|e| e.into_inner());
                let _ = writeln!(out, "{message}");
                let _ = out.flush();
            }
        };
        if is_initialize {
            // Later requests need the session id from this answer.
            forward();
        } else {
            // Run concurrently so a slow tool call does not block e.g. cancellations.
            std::thread::spawn(forward);
        }
    }
    Ok(())
}

/// POST one JSON-RPC message; return the JSON-RPC messages to write back.
fn forward_one(
    agent: &ureq::Agent,
    url: &str,
    session: &Mutex<Option<String>>,
    line: &str,
) -> Vec<String> {
    let id = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned());
    let session_id = session.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let mut request = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(sid) = &session_id {
        request = request.header("mcp-session-id", sid);
    }

    let mut response = match request.send(line) {
        Ok(r) => r,
        Err(err) => return error_reply(id, format!("BrowserMCP is not reachable: {err}")),
    };
    if let Some(sid) = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
    {
        *session.lock().unwrap_or_else(|e| e.into_inner()) = Some(sid.to_owned());
    }
    let status = response.status().as_u16();
    let is_sse = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .unwrap_or_default();

    if status == 202 || body.trim().is_empty() {
        return Vec::new(); // notification accepted, nothing to answer
    }
    if !(200..300).contains(&status) {
        return error_reply(id, format!("HTTP {status}: {}", body.trim()));
    }
    if is_sse {
        sse_messages(&body)
    } else {
        vec![body.trim().to_owned()]
    }
}

/// JSON-RPC messages contained in an SSE body (`data:` lines, skipping keep-alives).
pub fn sse_messages(body: &str) -> Vec<String> {
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain(std::iter::once("")) {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push_str(rest.trim_start());
        } else if line.is_empty() && !data.is_empty() {
            if serde_json::from_str::<Value>(&data).is_ok() {
                messages.push(std::mem::take(&mut data));
            } else {
                data.clear();
            }
        }
    }
    messages
}

fn error_reply(id: Option<Value>, message: String) -> Vec<String> {
    match id {
        // Requests must get an answer; notifications must not.
        Some(id) => vec![
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message}})
                .to_string(),
        ],
        None => {
            eprintln!("browsermcp mcp-stdio: {message}");
            Vec::new()
        }
    }
}

// ------------------------------------------------------------------ browser discovery

/// Return the MCP URL of a running browser, starting one if needed.
fn ensure_browser(data: &DataDir, api_port: u16) -> anyhow::Result<String> {
    let endpoint_file = data.runtime().join("endpoint.json");
    if let Some(url) = live_endpoint(&endpoint_file) {
        return Ok(url);
    }

    eprintln!("browsermcp mcp-stdio: browser not running, starting it");
    let exe = std::env::current_exe().context("locating browsermcp.exe")?;
    let _ = std::fs::remove_file(&endpoint_file);
    let mut cmd = Command::new(exe);
    cmd.arg("--data-dir")
        .arg(data.root())
        .arg("--api-port")
        .arg(api_port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The browser outlives us. If it inherited our stdin/stdout pipes, the MCP client
        // would never see EOF after we exit and would hang. Stop them from being inherited.
        disable_std_handle_inheritance();
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().context("starting the browser")?;

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(url) = live_endpoint(&endpoint_file) {
            return Ok(url);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("the browser did not start its API within 30 s (see data/logs)")
}

#[cfg(windows)]
fn disable_std_handle_inheritance() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain Win32 calls on our own std handles; failures are harmless.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// MCP URL from endpoint.json if its port accepts connections.
fn live_endpoint(endpoint_file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(endpoint_file).ok()?;
    let url = serde_json::from_str::<Value>(&text).ok()?["mcp"]
        .as_str()?
        .to_owned();
    let addr: SocketAddr = url
        .strip_prefix("http://")?
        .split('/')
        .next()?
        .parse()
        .ok()?;
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_body_yields_json_messages() {
        let body =
            "data: \nid: 0\nretry: 3000\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        assert_eq!(
            sse_messages(body),
            vec![r#"{"jsonrpc":"2.0","id":1,"result":{}}"#]
        );
    }

    #[test]
    fn multi_line_data_is_joined_and_last_event_flushed() {
        let body = "data: {\"a\":\ndata: 1}";
        assert_eq!(sse_messages(body), vec![r#"{"a":1}"#]);
    }

    #[test]
    fn errors_answer_requests_but_not_notifications() {
        let reply = error_reply(Some(json!(7)), "boom".into());
        assert!(reply[0].contains("\"id\":7") && reply[0].contains("boom"));
        assert!(error_reply(None, "boom".into()).is_empty());
    }

    #[test]
    fn missing_endpoint_is_not_live() {
        assert!(live_endpoint(Path::new("Z:/definitely/missing.json")).is_none());
    }
}
