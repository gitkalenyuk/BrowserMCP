//! WebSocket API at `GET /v1/ws`: JSON-RPC 2.0 calls + event notifications.
//!
//! Client → server, one JSON object per text frame:
//!
//! ```text
//! {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":{}}
//! ```
//!
//! * `method` is any command name, dotted (`tabs.list`) or wire (`tabs_list`); `params` is the
//!   argument object (missing/`null` = `{}`). Requests run concurrently: each one in its own
//!   task, so a slow `nav.goto` does not hold up a `tabs.list` sent after it. Responses carry the
//!   request `id` and may arrive in any order.
//! * A request without `id` is a JSON-RPC notification: it runs, but gets no response.
//! * Batches (JSON arrays) are not supported.
//!
//! Server → client:
//!
//! ```text
//! {"jsonrpc":"2.0","id":1,"result":{"tabs":[…]}}
//! {"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"tab 9 not found","data":{CmdError}}}
//! {"jsonrpc":"2.0","method":"command.finished","params":{…event data…,"seq":17}}
//! ```
//!
//! Error codes: `-32700` bad JSON, `-32600` not a JSON-RPC request, `-32601` unknown method,
//! `-32602` invalid params, `-32000` any other command error. `data` is always the `CmdError`
//! object (`{code:"TAB_NOT_FOUND",message,retry,hint?,candidates?}`), so clients can switch on
//! `data.code`.
//!
//! Events: two built-in methods manage which bus topics are pushed to this connection.
//! * `events.subscribe {"topics":["command.*","tab.*"]}` → `{"topics":[…all current…]}`
//! * `events.unsubscribe {"topics":["command.*"]}` (no `topics` = all) → `{"topics":[…left…]}`
//!
//! A pattern is an exact topic (`command.finished`), a prefix ending in `.*` (`tab.*`), or `*`.
//! New connections start subscribed to [`DEFAULT_TOPICS`] (the browser-state events the UI
//! renders); `command.*` must be asked for. If this connection falls too far behind the bus it
//! gets `{"method":"bus.lagged","params":{"missed":N}}` and should re-read state (e.g. `tabs.list`).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use bmcp_core::events::Event;
use bmcp_core::{Caller, CmdError, ErrorCode};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::state::ApiState;

/// Topics every new connection is subscribed to (what the UI needs without asking).
pub const DEFAULT_TOPICS: &[&str] = &["tab.*", "tabs.*", "agents.*", "workspace.*", "runs.*"];

/// Topic of the notification sent when this connection missed events.
pub const LAGGED_TOPIC: &str = "bus.lagged";

// JSON-RPC 2.0 error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// Any other command error; the `CmdError` code is in `error.data.code`.
const COMMAND_FAILED: i64 = -32000;

/// How many outgoing messages may queue up for one connection. When the client reads too
/// slowly the event forwarder waits here, falls behind the bus and reports `bus.lagged`.
const OUTBOX: usize = 256;

pub fn router() -> axum::Router<ApiState> {
    axum::Router::new().route("/v1/ws", get(upgrade))
}

async fn upgrade(State(state): State<ApiState>, ws: WebSocketUpgrade) -> Response {
    let conn = state.next_conn_id();
    ws.on_upgrade(move |socket| run_connection(state, conn, socket))
}

/// One connection. This task owns the socket: it reads requests and writes everything that
/// arrives on the outbox (responses from call tasks, notifications from the event forwarder).
async fn run_connection(state: ApiState, conn: u64, mut socket: WebSocket) {
    tracing::debug!(conn, "WebSocket connected");
    let subs = Arc::new(Mutex::new(Subscriptions::with_defaults()));
    let (outbox, mut outgoing) = mpsc::channel::<String>(OUTBOX);
    // Subscribe to the bus right away so no event is missed between connect and subscribe.
    let forwarder = tokio::spawn(forward_events(
        state.events.subscribe(),
        subs.clone(),
        outbox.clone(),
    ));

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text.as_str().to_owned(),
                    Some(Ok(Message::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
                    // Ping/pong frames are answered by the library.
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    // Close frame, end of stream or a broken connection.
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                };
                handle_message(&state, conn, &subs, &outbox, &text);
            }
            Some(message) = outgoing.recv() => {
                if socket.send(Message::Text(message.into())).await.is_err() {
                    break;
                }
            }
            () = state.shutdown.cancelled() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
        }
    }
    forwarder.abort();
    tracing::debug!(conn, "WebSocket closed");
}

/// Handle one incoming frame. Never awaits: answers go through the outbox from spawned tasks,
/// because this runs inside the loop that drains the outbox (awaiting here could deadlock).
fn handle_message(
    state: &ApiState,
    conn: u64,
    subs: &Arc<Mutex<Subscriptions>>,
    outbox: &mpsc::Sender<String>,
    text: &str,
) {
    let request = match parse_request(text) {
        Ok(request) => request,
        Err((id, code, err)) => return send_later(outbox, error_response(id, code, &err)),
    };

    match request.method.as_str() {
        "events.subscribe" | "events.unsubscribe" => {
            let subscribe = request.method == "events.subscribe";
            let result = update_subscriptions(subs, subscribe, &request.params);
            if let Some(id) = request.id {
                let message = match result {
                    Ok(topics) => success_response(id, json!({ "topics": topics })),
                    Err(err) => error_response(id, INVALID_PARAMS, &err),
                };
                send_later(outbox, message);
            }
        }
        _ => spawn_call(state, conn, outbox, request),
    }
}

/// Run a registry command in its own task and send the response when it is done.
fn spawn_call(state: &ApiState, conn: u64, outbox: &mpsc::Sender<String>, request: Request) {
    let unknown_method = state.registry.get(&request.method).is_none();
    let ctx = state.ctx(Caller::Ws { conn });
    let call = state.registry.call(ctx, &request.method, request.params);
    let outbox = outbox.clone();
    let method = request.method;
    tokio::spawn(async move {
        // The inner spawn turns a panicking command into an error instead of a lost answer.
        let result = tokio::spawn(call).await.unwrap_or_else(|join_err| {
            Err(CmdError::internal(format!(
                "command {method} panicked: {join_err}"
            )))
        });
        // No id = notification: the caller does not want an answer.
        let Some(id) = request.id else { return };
        let message = match result {
            Ok(output) => success_response(id, output),
            Err(err) => {
                let code = if unknown_method {
                    METHOD_NOT_FOUND
                } else if err.code == ErrorCode::InvalidParams {
                    INVALID_PARAMS
                } else {
                    COMMAND_FAILED
                };
                error_response(id, code, &err)
            }
        };
        let _ = outbox.send(message).await;
    });
}

/// Queue `message` without blocking the connection loop.
fn send_later(outbox: &mpsc::Sender<String>, message: String) {
    let outbox = outbox.clone();
    tokio::spawn(async move {
        let _ = outbox.send(message).await;
    });
}

/// Push matching bus events to the connection until it closes.
async fn forward_events(
    mut events: broadcast::Receiver<Event>,
    subs: Arc<Mutex<Subscriptions>>,
    outbox: mpsc::Sender<String>,
) {
    loop {
        let message = match events.recv().await {
            Ok(event) => {
                if !lock(&subs).matches(&event.topic) {
                    continue;
                }
                notification(&event.topic, with_seq(event.data, event.seq))
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                if lock(&subs).is_empty() {
                    continue; // not interested in anything, so nothing was really missed
                }
                notification(LAGGED_TOPIC, json!({ "missed": missed }))
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if outbox.send(message).await.is_err() {
            break; // connection gone
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Subscriptions

/// The topic patterns one connection listens to.
#[derive(Debug, Default)]
struct Subscriptions {
    patterns: BTreeSet<String>,
}

impl Subscriptions {
    fn with_defaults() -> Self {
        Subscriptions {
            patterns: DEFAULT_TOPICS.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn matches(&self, topic: &str) -> bool {
        self.patterns.iter().any(|p| topic_matches(p, topic))
    }

    fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    fn list(&self) -> Vec<String> {
        self.patterns.iter().cloned().collect()
    }
}

/// `*` = everything; `tab.*` = `tab.created`, `tab.x.y` (but not `tabs.reset`); else exact.
fn topic_matches(pattern: &str, topic: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix(".*") {
        Some(prefix) => topic
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.')),
        None => pattern == topic,
    }
}

/// Lowercase dotted names, optionally ending in `.*`, or a lone `*`.
fn valid_pattern(pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let name = pattern.strip_suffix(".*").unwrap_or(pattern);
    !name.is_empty()
        && name.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// `events.subscribe` / `events.unsubscribe`; returns the patterns now active.
fn update_subscriptions(
    subs: &Mutex<Subscriptions>,
    subscribe: bool,
    params: &Value,
) -> Result<Vec<String>, CmdError> {
    let topics = match params.get("topics") {
        None | Some(Value::Null) if !subscribe => None, // unsubscribe from everything
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|item| match item.as_str() {
                    Some(p) if valid_pattern(p) => Ok(p.to_owned()),
                    _ => Err(CmdError::invalid_params(format!(
                        "bad topic pattern {item}: use names like \"command.finished\", \
                         \"tab.*\" or \"*\""
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        _ => {
            return Err(CmdError::invalid_params(
                "params must be {\"topics\": [\"command.*\", …]}",
            ));
        }
    };

    let mut subs = lock(subs);
    match topics {
        Some(topics) if subscribe => subs.patterns.extend(topics),
        Some(topics) => topics.iter().for_each(|t| {
            subs.patterns.remove(t);
        }),
        None => subs.patterns.clear(),
    }
    Ok(subs.list())
}

fn lock(subs: &Mutex<Subscriptions>) -> std::sync::MutexGuard<'_, Subscriptions> {
    // A poisoned lock only means a panic elsewhere; the set itself is still usable.
    subs.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// JSON-RPC messages

/// A parsed JSON-RPC request. `id: None` = notification (no response wanted).
#[derive(Debug)]
struct Request {
    id: Option<Value>,
    method: String,
    params: Value,
}

/// Parse a frame; on failure return `(id to answer with, JSON-RPC code, error)`.
fn parse_request(text: &str) -> Result<Request, (Value, i64, CmdError)> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        (
            Value::Null,
            PARSE_ERROR,
            CmdError::invalid_params(format!("not valid JSON: {e}")),
        )
    })?;
    let Value::Object(mut obj) = value else {
        let what = if value.is_array() {
            "batch requests (arrays) are not supported; send one object per frame"
        } else {
            "a JSON-RPC request must be a JSON object"
        };
        return Err((Value::Null, INVALID_REQUEST, CmdError::invalid_params(what)));
    };
    let id = obj.remove("id");
    let reply_id = id.clone().unwrap_or(Value::Null);
    let invalid = |msg: &str| {
        Err((
            reply_id.clone(),
            INVALID_REQUEST,
            CmdError::invalid_params(msg),
        ))
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return invalid("missing \"jsonrpc\": \"2.0\"");
    }
    let Some(Value::String(method)) = obj.remove("method") else {
        return invalid("missing \"method\" (a command name like \"tabs.list\")");
    };
    let params = obj.remove("params").unwrap_or(Value::Null);
    Ok(Request { id, method, params })
}

fn success_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Value, code: i64, err: &CmdError) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": err.message, "data": err },
    })
    .to_string()
}

fn notification(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

/// Event data plus its bus `seq`: `{…data, "seq": 17}` (non-object data goes in `value`).
fn with_seq(data: Value, seq: u64) -> Value {
    match data {
        Value::Object(mut obj) => {
            obj.insert("seq".into(), json!(seq));
            Value::Object(obj)
        }
        other => json!({ "value": other, "seq": seq }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_patterns() {
        assert!(topic_matches("*", "anything.at.all"));
        assert!(topic_matches("tab.*", "tab.created"));
        assert!(topic_matches("tab.*", "tab.a.b"));
        assert!(!topic_matches("tab.*", "tabs.reset"));
        assert!(!topic_matches("tab.*", "tab"));
        assert!(topic_matches("command.finished", "command.finished"));
        assert!(!topic_matches("command.finished", "command.started"));

        assert!(valid_pattern("command.*") && valid_pattern("*") && valid_pattern("tab"));
        for bad in ["", ".*", "Tab.*", "tab..x", "tab*", "a.*.b"] {
            assert!(!valid_pattern(bad), "{bad}");
        }
    }

    #[test]
    fn subscribe_and_unsubscribe() {
        let subs = Mutex::new(Subscriptions::default());
        let topics =
            update_subscriptions(&subs, true, &json!({ "topics": ["command.*", "tab.*"] }))
                .unwrap();
        assert_eq!(topics, ["command.*", "tab.*"]);
        let topics = update_subscriptions(&subs, false, &json!({ "topics": ["tab.*"] })).unwrap();
        assert_eq!(topics, ["command.*"]);
        assert!(update_subscriptions(&subs, true, &json!({ "topics": ["Bad"] })).is_err());
        assert!(update_subscriptions(&subs, true, &json!({})).is_err());
        assert!(
            update_subscriptions(&subs, false, &Value::Null)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn request_parsing() {
        let r = parse_request(r#"{"jsonrpc":"2.0","id":"a","method":"tabs.list"}"#).unwrap();
        assert_eq!(
            (r.id, r.method, r.params),
            (Some(json!("a")), "tabs.list".into(), Value::Null)
        );
        let r = parse_request(r#"{"jsonrpc":"2.0","method":"x.y","params":{"a":1}}"#).unwrap();
        assert_eq!(r.id, None);

        let (id, code, _) = parse_request("{oops").unwrap_err();
        assert_eq!((id, code), (Value::Null, PARSE_ERROR));
        let (_, code, _) = parse_request("[]").unwrap_err();
        assert_eq!(code, INVALID_REQUEST);
        let (id, code, _) = parse_request(r#"{"id":5,"method":"tabs.list"}"#).unwrap_err();
        assert_eq!((id, code), (json!(5), INVALID_REQUEST));
    }

    #[test]
    fn seq_is_added() {
        assert_eq!(with_seq(json!({ "a": 1 }), 3), json!({ "a": 1, "seq": 3 }));
        assert_eq!(with_seq(json!(5), 4), json!({ "value": 5, "seq": 4 }));
    }
}
