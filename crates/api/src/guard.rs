//! Host / Origin guard for the REST and WebSocket routes (`/v1/*`, `/openapi.json`).
//!
//! `/mcp` has its own guard inside rmcp (loopback `Host`, every `Origin` rejected), so this
//! middleware is only put in front of the other routes. The rules:
//!
//! 1. **Host** must be a loopback name — `localhost`, `127.0.0.1` or `[::1]`, any port.
//!    A web page that tricks DNS into pointing `evil.example` at 127.0.0.1 ("DNS rebinding")
//!    still sends `Host: evil.example`, so it is refused.
//! 2. **Origin**:
//!    * no `Origin` header → allowed. CLI tools, curl, scripts and the script host don't send one;
//!    * `Origin: bmcp://ui` (the browser's own UI, future custom scheme) → allowed;
//!    * `Origin: http://localhost:5173` / `http://127.0.0.1:5173` (Vite dev server) → allowed only
//!      when the browser runs with `BMCP_DEV_UI=1`;
//!    * any other `Origin` → 403. Every web page sends `Origin` on `fetch` and WebSocket
//!      handshakes, so pages open in any browser (including BrowserMCP itself) can't use the API.
//!
//! CORS headers are only ever added for the allowed UI origins; for everyone else the browser's
//! same-origin policy stays fully in force.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The browser's own UI, served from the `bmcp://` custom scheme (planned).
pub const UI_ORIGIN: &str = "bmcp://ui";
/// Vite dev server origins, allowed only in dev mode (`BMCP_DEV_UI=1`).
pub const DEV_UI_ORIGINS: &[&str] = &["http://localhost:5173", "http://127.0.0.1:5173"];
/// Environment variable that turns on dev mode.
pub const DEV_UI_ENV: &str = "BMCP_DEV_UI";

/// Which `Origin` values may call the API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OriginPolicy {
    /// Also allow the Vite dev server origins.
    pub dev_ui: bool,
}

impl OriginPolicy {
    /// Dev mode when `BMCP_DEV_UI=1` (or `true`) is set in the environment.
    pub fn from_env() -> Self {
        let dev_ui = std::env::var(DEV_UI_ENV)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        OriginPolicy { dev_ui }
    }

    /// True when a request carrying this exact `Origin` value may call the API.
    pub fn allows_origin(&self, origin: &str) -> bool {
        origin == UI_ORIGIN || (self.dev_ui && DEV_UI_ORIGINS.contains(&origin))
    }
}

/// True for `localhost`, `127.0.0.1` and `[::1]`, with or without `:port`.
pub fn host_is_loopback(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    // IPv6 is written in brackets in a Host header: `[::1]:47800`.
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            // After `]` only `:port` (or nothing) may follow.
            Some((name, tail)) if tail.is_empty() || is_port(tail) => name.to_string(),
            _ => return false,
        }
    } else {
        match host.split_once(':') {
            Some((name, port)) if is_port(&format!(":{port}")) => name.to_string(),
            Some(_) => return false,
            None => host,
        }
    };
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1")
}

/// `":47800"` → true (a colon followed by 1–5 digits).
fn is_port(tail: &str) -> bool {
    tail.strip_prefix(':')
        .is_some_and(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The `Host` header, or the URI authority (HTTP/2 style) when there is no header.
/// Several `Host` headers are invalid HTTP (RFC 9112 §3.2): `Some("")`, i.e. refused.
fn request_host(req: &Request) -> Option<String> {
    let mut hosts = req.headers().get_all(header::HOST).iter();
    match (hosts.next(), hosts.next()) {
        (Some(_), Some(_)) => Some(String::new()),
        (Some(value), None) => Some(String::from_utf8_lossy(value.as_bytes()).into_owned()),
        (None, _) => req.uri().authority().map(|a| a.to_string()),
    }
}

/// The axum middleware. Use with `axum::middleware::from_fn_with_state(policy, guard)`.
pub async fn guard(State(policy): State<OriginPolicy>, req: Request, next: Next) -> Response {
    // 1. Host.
    match request_host(&req) {
        Some(host) if host_is_loopback(&host) => {}
        Some(host) => return forbidden(&format!("Host '{host}' is not allowed")),
        None => return forbidden("missing Host header"),
    }

    // 2. Origin. No header = not a web page = allowed, without CORS headers.
    let Some(origin) = req.headers().get(header::ORIGIN).cloned() else {
        return next.run(req).await;
    };
    let allowed = origin.to_str().is_ok_and(|o| policy.allows_origin(o));
    if !allowed {
        let shown = String::from_utf8_lossy(origin.as_bytes()).into_owned();
        return forbidden(&format!("Origin '{shown}' is not allowed"));
    }

    // CORS preflight from an allowed UI origin (e.g. `fetch` POST with a JSON body).
    if req.method() == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        let mut response = StatusCode::NO_CONTENT.into_response();
        let headers = response.headers_mut();
        add_cors_headers(headers, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        return response;
    }

    let mut response = next.run(req).await;
    add_cors_headers(response.headers_mut(), origin);
    response
}

fn add_cors_headers(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
}

/// 403 with a small JSON body in the same shape as `CmdError`.
fn forbidden(message: &str) -> Response {
    tracing::warn!(%message, "API request refused");
    let body = json!({
        "code": "FORBIDDEN",
        "message": message,
        "retry": "no",
        "hint": "The API accepts requests from localhost without an Origin header \
                 (CLI tools, scripts) and from the BrowserMCP UI only.",
    });
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::FORBIDDEN.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for ok in [
            "localhost",
            "localhost:47800",
            "LOCALHOST:1",
            "127.0.0.1",
            "127.0.0.1:47800",
            "[::1]",
            "[::1]:47800",
        ] {
            assert!(host_is_loopback(ok), "{ok} should be allowed");
        }
        for bad in [
            "evil.example",
            "evil.example:47800",
            "127.0.0.2",
            "localhost.evil.example",
            "localhost:abc",
            "localhost:",
            "[::1]x",
            "[::2]:47800",
            "0.0.0.0:47800",
            "",
        ] {
            assert!(!host_is_loopback(bad), "{bad} should be refused");
        }
    }

    #[test]
    fn origins() {
        let prod = OriginPolicy { dev_ui: false };
        let dev = OriginPolicy { dev_ui: true };
        assert!(prod.allows_origin("bmcp://ui"));
        assert!(!prod.allows_origin("http://localhost:5173"));
        assert!(dev.allows_origin("http://localhost:5173"));
        assert!(dev.allows_origin("http://127.0.0.1:5173"));
        for bad in [
            "https://evil.example",
            "null",
            "http://localhost:5174",
            "bmcp://ui.evil",
        ] {
            assert!(!prod.allows_origin(bad) && !dev.allows_origin(bad), "{bad}");
        }
    }
}
