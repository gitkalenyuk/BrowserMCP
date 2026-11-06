//! REST, WebSocket and Host/Origin guard tests against a `FakeBackend`.
//!
//! The REST/guard tests send requests straight into the router with `oneshot` (no socket);
//! the WebSocket test starts the real server on a free port.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use bmcp_api::{ApiState, OriginPolicy, api_router};
use bmcp_core::testing::FakeBackend;
use bmcp_core::{EventBus, Registry};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn registry() -> Arc<Registry> {
    let mut registry = Registry::new();
    bmcp_core::commands::register_builtin(&mut registry);
    bmcp_api::commands::register(&mut registry);
    Arc::new(registry)
}

fn router(policy: OriginPolicy) -> axum::Router {
    let backend = FakeBackend::with_tabs(&["https://a.test/"]);
    let state = ApiState::new(
        registry(),
        backend,
        EventBus::new(),
        CancellationToken::new(),
    );
    api_router(state, policy)
}

/// A request with a loopback Host; `extra` adds/overrides headers.
fn request(method: &str, uri: &str, body: &str, extra: &[(&str, &str)]) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:47800");
    let mut req = builder.body(Body::from(body.to_string())).unwrap();
    for (name, value) in extra {
        // `insert` replaces, so `("host", …)` overrides the default Host.
        let name = header::HeaderName::from_bytes(name.as_bytes()).unwrap();
        req.headers_mut().insert(name, value.parse().unwrap());
    }
    req
}

/// Send one request; return status, headers and the body parsed as JSON (Null if empty).
async fn send(
    router: axum::Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = router.oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

async fn get(uri: &str, extra: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, Value) {
    send(
        router(OriginPolicy::default()),
        request("GET", uri, "", extra),
    )
    .await
}

// ------------------------------------------------------------------------------- guard: Host

#[tokio::test]
async fn loopback_hosts_are_allowed() {
    for host in [
        "127.0.0.1:47800",
        "localhost:47800",
        "localhost",
        "[::1]:47800",
    ] {
        let (status, _, body) = get("/v1/health", &[("host", host)]).await;
        assert_eq!(status, StatusCode::OK, "Host {host}");
        assert_eq!(body["ok"], true);
    }
}

#[tokio::test]
async fn other_hosts_are_refused() {
    for host in [
        "evil.example",
        "evil.example:47800",
        "192.168.1.5:47800",
        "127.0.0.1.nip.io",
    ] {
        let (status, _, body) = get("/v1/health", &[("host", host)]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "Host {host}");
        assert_eq!(body["code"], "FORBIDDEN");
    }
    // POST and WS routes are guarded too.
    let req = request("POST", "/v1/cmd/tabs.list", "", &[("host", "evil.example")]);
    let (status, _, _) = send(router(OriginPolicy::default()), req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = get("/v1/ws", &[("host", "evil.example")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn missing_host_is_refused() {
    let req = Request::builder()
        .uri("/v1/health")
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = send(router(OriginPolicy::default()), req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------------------- guard: Origin

#[tokio::test]
async fn no_origin_is_allowed_without_cors_headers() {
    let (status, headers, _) = get("/v1/health", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
}

#[tokio::test]
async fn ui_origin_is_allowed_with_cors_headers() {
    let (status, headers, _) = get("/v1/health", &[("origin", "bmcp://ui")]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "bmcp://ui");
    assert_eq!(headers[header::VARY], "Origin");
}

#[tokio::test]
async fn foreign_origins_are_refused() {
    for origin in [
        "https://evil.example",
        "null",
        "http://127.0.0.1:47800",
        "http://localhost:5173", // dev server, but dev mode is off
    ] {
        let (status, headers, body) = get("/v1/commands", &[("origin", origin)]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "Origin {origin}");
        assert_eq!(body["code"], "FORBIDDEN");
        assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    }
    let req = request(
        "POST",
        "/v1/cmd/nav.goto",
        r#"{"url":"https://x.test/"}"#,
        &[("origin", "https://evil.example")],
    );
    let (status, _, _) = send(router(OriginPolicy::default()), req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dev_ui_origins_only_in_dev_mode() {
    let dev = OriginPolicy { dev_ui: true };
    for origin in ["http://localhost:5173", "http://127.0.0.1:5173"] {
        let req = request("GET", "/v1/health", "", &[("origin", origin)]);
        let (status, headers, _) = send(router(dev), req).await;
        assert_eq!(status, StatusCode::OK, "Origin {origin}");
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
    }
    let req = request(
        "GET",
        "/v1/health",
        "",
        &[("origin", "https://evil.example")],
    );
    let (status, _, _) = send(router(dev), req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cors_preflight_for_ui_origin() {
    let preflight = |origin: &'static str| {
        request(
            "OPTIONS",
            "/v1/cmd/tabs.list",
            "",
            &[
                ("origin", origin),
                ("access-control-request-method", "POST"),
                ("access-control-request-headers", "content-type"),
            ],
        )
    };
    let (status, headers, _) = send(router(OriginPolicy::default()), preflight("bmcp://ui")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "bmcp://ui");
    assert!(
        headers[header::ACCESS_CONTROL_ALLOW_METHODS]
            .to_str()
            .unwrap()
            .contains("POST")
    );
    let (status, _, _) = send(
        router(OriginPolicy::default()),
        preflight("https://evil.example"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------------------------- REST

#[tokio::test]
async fn health() {
    let (status, _, body) = get("/v1/health", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "BrowserMCP");
    assert!(body["commands"].as_u64().unwrap() >= 5);
}

#[tokio::test]
async fn list_commands_with_schemas_and_toolsets() {
    let (status, _, body) = get("/v1/commands", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let commands = body["commands"].as_array().unwrap();
    let goto = commands.iter().find(|c| c["name"] == "nav.goto").unwrap();
    assert_eq!(goto["wire_name"], "nav_goto");
    assert_eq!(goto["args_schema"]["type"], "object");
    assert_eq!(goto["output_schema"]["type"], "object");

    let (_, _, body) = get("/v1/commands?toolsets=core", &[]).await;
    let names: Vec<&str> = body["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"tabs.list") && names.contains(&"nav.goto"));
    // No built-in command is in the `net` toolset (yet).
    let (_, _, body) = get("/v1/commands?toolsets=net", &[]).await;
    assert_eq!(body["commands"], json!([]));

    let (status, _, body) = get("/v1/commands?toolsets=nope", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_PARAMS");
}

async fn post(uri: &str, body: &str) -> (StatusCode, Value) {
    let (status, _, body) = send(
        router(OriginPolicy::default()),
        request("POST", uri, body, &[("content-type", "application/json")]),
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn call_commands() {
    // Empty body = no arguments; dotted and wire names both work.
    for uri in ["/v1/cmd/tabs.list", "/v1/cmd/tabs_list"] {
        let (status, body) = post(uri, "").await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(body["tabs"][0]["url"], "https://a.test/");
    }
    let (status, body) = post("/v1/cmd/nav.goto", r#"{"url":"https://b.test/"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["url"], "https://b.test/");
}

#[tokio::test]
async fn command_errors_map_to_statuses() {
    // Unknown command → 404 with candidates of the same group.
    let (status, body) = post("/v1/cmd/tabs.lst", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "NOT_FOUND");
    assert!(
        body["candidates"]
            .as_array()
            .unwrap()
            .contains(&json!("tabs.list"))
    );

    // Schema violation → 400 with the field path.
    let (status, body) = post("/v1/cmd/nav.goto", r#"{"url":"https://b.test/","tab":"x"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_PARAMS");
    assert_eq!(body["retry"], "no");
    assert!(body["message"].as_str().unwrap().contains("`tab`"));

    // Not JSON → 400.
    let (status, body) = post("/v1/cmd/tabs.list", "{oops").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_PARAMS");

    // Arguments must be an object → 400.
    let (status, _) = post("/v1/cmd/tabs.list", "[1]").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Backend error → 404 TAB_NOT_FOUND with a hint.
    let (status, body) = post("/v1/cmd/nav.goto", r#"{"url":"https://b.test/","tab":9}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "TAB_NOT_FOUND");
    assert!(body["hint"].is_string());
}

#[tokio::test]
async fn openapi_document() {
    let (status, _, doc) = get("/openapi.json", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["servers"][0]["url"], "http://127.0.0.1:47800");
    let op = &doc["paths"]["/v1/cmd/tabs.open"]["post"];
    assert_eq!(op["operationId"], "tabs_open");
    assert_eq!(op["x-bmcp-mutates-tab"], false);
    assert!(doc["components"]["schemas"]["CmdError"].is_object());
}

// -------------------------------------------------------------------------------- WebSocket

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send_json(socket: &mut Socket, value: Value) {
    socket.send(Message::text(value.to_string())).await.unwrap();
}

/// Next JSON message from the server (fails the test after 30 s: only a real hang, never a
/// busy machine, gets there).
async fn next_json(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("no message within 30 s")
            .expect("socket closed")
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

/// Read until the response with this id; notifications seen on the way are collected.
async fn response(socket: &mut Socket, id: i64, seen: &mut Vec<Value>) -> Value {
    loop {
        let msg = next_json(socket).await;
        if msg["id"] == json!(id) {
            return msg;
        }
        seen.push(msg);
    }
}

#[tokio::test]
async fn websocket_json_rpc_and_events() {
    let shutdown = CancellationToken::new();
    let backend = FakeBackend::with_tabs(&["https://a.test/"]);
    let events = EventBus::new();
    let server = bmcp_api::serve(backend, registry(), events.clone(), 0, shutdown.clone())
        .await
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(server.ws_url())
        .await
        .unwrap();
    let mut seen = Vec::new();

    // A plain call.
    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":1,"method":"tabs.list","params":{}}),
    )
    .await;
    let msg = response(&mut socket, 1, &mut seen).await;
    assert_eq!(msg["result"]["tabs"][0]["id"], 1);

    // Errors: unknown method, invalid params, command failure, bad JSON.
    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":2,"method":"tabs.lst"}),
    )
    .await;
    let msg = response(&mut socket, 2, &mut seen).await;
    assert_eq!(msg["error"]["code"], -32601);
    assert_eq!(msg["error"]["data"]["code"], "NOT_FOUND");

    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":3,"method":"nav_goto","params":{"tab":"x"}}),
    )
    .await;
    let msg = response(&mut socket, 3, &mut seen).await;
    assert_eq!(msg["error"]["code"], -32602);
    assert_eq!(msg["error"]["data"]["code"], "INVALID_PARAMS");

    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":4,"method":"nav.goto","params":{"url":"https://b.test/","tab":9}}),
    )
    .await;
    let msg = response(&mut socket, 4, &mut seen).await;
    assert_eq!(msg["error"]["code"], -32000);
    assert_eq!(msg["error"]["data"]["code"], "TAB_NOT_FOUND");

    socket.send(Message::text("{oops")).await.unwrap();
    let msg = next_json(&mut socket).await;
    assert_eq!(msg["id"], Value::Null);
    assert_eq!(msg["error"]["code"], -32700);

    // No command events before subscribing (only the UI defaults are on).
    assert!(seen.is_empty(), "unexpected notifications: {seen:?}");

    // Subscribe to command events and see our own call's events.
    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":5,"method":"events.subscribe","params":{"topics":["command.*"]}}),
    )
    .await;
    let msg = response(&mut socket, 5, &mut seen).await;
    assert!(
        msg["result"]["topics"]
            .as_array()
            .unwrap()
            .contains(&json!("command.*"))
    );

    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":6,"method":"tabs.open","params":{"url":"https://c.test/"}}),
    )
    .await;
    let msg = response(&mut socket, 6, &mut seen).await;
    assert_eq!(msg["result"]["tab"], 2);
    // The finished event may come right after the response.
    while !seen.iter().any(|m| m["method"] == "command.finished") {
        seen.push(next_json(&mut socket).await);
    }
    let finished = seen
        .iter()
        .find(|m| m["method"] == "command.finished")
        .unwrap();
    assert_eq!(finished["params"]["name"], "tabs.open");
    assert_eq!(finished["params"]["ok"], true);
    assert_eq!(finished["params"]["caller"]["kind"], "ws");
    assert!(finished["params"]["seq"].as_u64().unwrap() > 0);
    assert!(finished.get("id").is_none());

    // Events from other sources, e.g. a future `tab.created` from the CEF layer.
    events.emit("tab.created", json!({"tab": {"id": 2}, "index": 1}));
    let msg = loop {
        let msg = next_json(&mut socket).await;
        if msg["method"] == "tab.created" {
            break msg;
        }
    };
    assert_eq!(msg["params"]["index"], 1);

    // Unsubscribe from everything: no more notifications, calls still answered.
    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":7,"method":"events.unsubscribe"}),
    )
    .await;
    let msg = response(&mut socket, 7, &mut Vec::new()).await;
    assert_eq!(msg["result"]["topics"], json!([]));
    events.emit("tab.created", json!({}));
    send_json(
        &mut socket,
        json!({"jsonrpc":"2.0","id":8,"method":"system.ping"}),
    )
    .await;
    let mut after = Vec::new();
    let msg = response(&mut socket, 8, &mut after).await;
    assert_eq!(msg["result"]["name"], "BrowserMCP");
    assert!(after.is_empty(), "unexpected notifications: {after:?}");

    // Shutdown closes the connection.
    shutdown.cancel();
    let closed = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "socket not closed on shutdown");
}

#[tokio::test]
async fn websocket_requests_run_concurrently() {
    let shutdown = CancellationToken::new();
    let server = bmcp_api::serve(
        FakeBackend::with_tabs(&["https://a.test/"]),
        registry(),
        EventBus::new(),
        0,
        shutdown.clone(),
    )
    .await
    .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(server.ws_url())
        .await
        .unwrap();
    for id in 1..=20 {
        send_json(
            &mut socket,
            json!({"jsonrpc":"2.0","id":id,"method":"system.ping"}),
        )
        .await;
    }
    let mut ids = Vec::new();
    while ids.len() < 20 {
        let msg = next_json(&mut socket).await;
        ids.push(msg["id"].as_i64().unwrap());
    }
    ids.sort();
    assert_eq!(ids, (1..=20).collect::<Vec<_>>());
    shutdown.cancel();
}

#[tokio::test]
async fn websocket_refuses_foreign_origin() {
    let shutdown = CancellationToken::new();
    let server = bmcp_api::serve(
        FakeBackend::new(),
        registry(),
        EventBus::new(),
        0,
        shutdown.clone(),
    )
    .await
    .unwrap();
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = server.ws_url().into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    match err {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 403)
        }
        other => panic!("expected HTTP 403, got {other}"),
    }

    // The UI origin may connect.
    let mut req = server.ws_url().into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", "bmcp://ui".parse().unwrap());
    tokio_tungstenite::connect_async(req).await.unwrap();
    shutdown.cancel();
}
