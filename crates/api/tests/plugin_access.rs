//! Plugin connections (`/v1/ws?plugin=…&token=…`) and the webhook route.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use bmcp_api::access::{PluginAuth, PluginAuthHandle, PluginGrant};
use bmcp_api::{ApiState, OriginPolicy, api_router};
use bmcp_core::testing::FakeBackend;
use bmcp_core::{CmdError, Command, Ctx, EventBus, Meta, Registry, Toolset};
use futures_util::{SinkExt, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// Plugin "hello" with token "good" may list tabs and see `tab.*`.
struct OneToken;

impl PluginAuth for OneToken {
    fn authenticate(&self, plugin: &str, token: &str) -> Option<PluginGrant> {
        (plugin == "hello" && token == "good").then(|| PluginGrant {
            name: "hello".into(),
            commands: vec!["tabs.list".into(), "system.*".into()],
            events: vec!["tab.*".into()],
        })
    }
}

fn registry(with_auth: bool, with_hook: bool) -> Arc<Registry> {
    let mut registry = Registry::new();
    bmcp_core::commands::register_builtin(&mut registry);
    bmcp_api::commands::register(&mut registry);
    if with_hook {
        registry.add::<FakeFire>();
    }
    if with_auth {
        registry
            .extensions()
            .insert(PluginAuthHandle(Arc::new(OneToken)));
    }
    Arc::new(registry)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn call(socket: &mut Socket, id: i64, method: &str, params: Value) -> Value {
    let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    socket.send(Message::text(req.to_string())).await.unwrap();
    loop {
        let msg = next_json(socket).await.expect("no response");
        if msg["id"] == json!(id) {
            return msg;
        }
    }
}

/// Next JSON message, or `None` when nothing arrives within `wait`.
async fn next_json_within(socket: &mut Socket, wait: Duration) -> Option<Value> {
    loop {
        let message = tokio::time::timeout(wait, socket.next())
            .await
            .ok()??
            .ok()?;
        if let Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).ok();
        }
    }
}

async fn next_json(socket: &mut Socket) -> Option<Value> {
    next_json_within(socket, Duration::from_secs(5)).await
}

#[tokio::test]
async fn plugin_connections_are_restricted() {
    let shutdown = CancellationToken::new();
    let events = EventBus::new();
    let server = bmcp_api::serve(
        FakeBackend::with_tabs(&["https://a.test/"]),
        registry(true, false),
        events.clone(),
        0,
        shutdown.clone(),
    )
    .await
    .unwrap();

    // Wrong token, unknown plugin: refused before the upgrade (not an unrestricted socket).
    for query in [
        "?plugin=hello&token=bad",
        "?plugin=other&token=good",
        "?plugin=hello",
    ] {
        let err = tokio_tungstenite::connect_async(format!("{}{query}", server.ws_url()))
            .await
            .unwrap_err();
        match err {
            tokio_tungstenite::tungstenite::Error::Http(r) => {
                assert_eq!(r.status().as_u16(), 403, "{query}")
            }
            other => panic!("{query}: expected HTTP 403, got {other}"),
        }
    }

    let url = format!("{}?plugin=hello&token=good", server.ws_url());
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    // Allowed command.
    let msg = call(&mut socket, 1, "tabs.list", json!({})).await;
    assert_eq!(msg["result"]["tabs"][0]["id"], 1, "{msg}");
    // Not allowed: dotted and wire name.
    for (id, method) in [(2, "tabs.open"), (3, "tabs_open")] {
        let msg = call(&mut socket, id, method, json!({"url":"https://b.test/"})).await;
        assert_eq!(
            msg["error"]["data"]["code"], "NEEDS_APPROVAL",
            "{method}: {msg}"
        );
        assert!(
            msg["error"]["message"]
                .as_str()
                .unwrap()
                .contains("tabs.open")
        );
    }

    // No default subscriptions: a tab event before subscribing is not delivered.
    events.emit("tab.created", json!({"id": 5}));
    assert!(
        next_json_within(&mut socket, Duration::from_millis(300))
            .await
            .is_none()
    );

    // Even after subscribing to everything, only granted topics + own palette commands come.
    let msg = call(&mut socket, 4, "events.subscribe", json!({"topics":["*"]})).await;
    assert_eq!(msg["result"]["topics"], json!(["*"]));
    events.emit("net.request", json!({"id": 1}));
    events.emit("command.finished", json!({"name": "x"}));
    events.emit("plugin.command", json!({"plugin": "other", "id": "a"}));
    events.emit("plugin.command", json!({"plugin": "hello", "id": "b"}));
    events.emit("tab.updated", json!({"id": 6}));
    let first = next_json(&mut socket).await.unwrap();
    assert_eq!(first["method"], "plugin.command", "{first}");
    assert_eq!(first["params"]["id"], "b");
    let second = next_json(&mut socket).await.unwrap();
    assert_eq!(second["method"], "tab.updated", "{second}");
    assert!(
        next_json_within(&mut socket, Duration::from_millis(300))
            .await
            .is_none()
    );

    shutdown.cancel();
}

#[tokio::test]
async fn plugin_queries_need_the_auth_service() {
    // Without a PluginAuth service no plugin can connect at all.
    let shutdown = CancellationToken::new();
    let server = bmcp_api::serve(
        FakeBackend::new(),
        registry(false, false),
        EventBus::new(),
        0,
        shutdown.clone(),
    )
    .await
    .unwrap();
    let url = format!("{}?plugin=hello&token=good", server.ws_url());
    assert!(tokio_tungstenite::connect_async(url).await.is_err());
    shutdown.cancel();
}

// ------------------------------------------------------------------------------ webhooks

/// Stands in for `bmcp-plugins`' `trigger.webhook_fire`: echoes what the route passed.
struct FakeFire;

#[derive(Deserialize, JsonSchema)]
struct FireArgs {
    id: String,
    signature: String,
    body: String,
}

#[derive(Serialize, JsonSchema)]
struct FireOutput {
    run_id: String,
    echo: Value,
}

impl Command for FakeFire {
    const NAME: &'static str = bmcp_api::rest::WEBHOOK_COMMAND;
    type Args = FireArgs;
    type Output = FireOutput;

    fn meta() -> Meta {
        Meta::new("test double", Toolset::Hidden)
    }

    async fn run(_ctx: Ctx, args: FireArgs) -> Result<FireOutput, CmdError> {
        if args.id == "missing" {
            return Err(CmdError::new(bmcp_core::ErrorCode::NotFound, "no hook"));
        }
        Ok(FireOutput {
            run_id: "run-1".into(),
            echo: json!({ "id": args.id, "signature": args.signature, "body": args.body }),
        })
    }
}

async fn post_hook(with_hook: bool, uri: &str, extra: &[(&str, &str)]) -> (StatusCode, Value) {
    let state = ApiState::new(
        registry(false, with_hook),
        FakeBackend::new(),
        EventBus::new(),
        CancellationToken::new(),
    );
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:47800")
        .body(Body::from(r#"{"a": 1}"#))
        .unwrap();
    for (name, value) in extra {
        let name = header::HeaderName::from_bytes(name.as_bytes()).unwrap();
        req.headers_mut().insert(name, value.parse().unwrap());
    }
    let response = api_router(state, OriginPolicy::default())
        .oneshot(req)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn webhook_route_passes_raw_body_and_signature() {
    let (status, body) =
        post_hook(true, "/v1/hooks/h1", &[("x-bmcp-signature", "sha256=abc")]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["run_id"], "run-1");
    assert_eq!(
        body["echo"],
        json!({ "id": "h1", "signature": "sha256=abc", "body": "{\"a\": 1}" })
    );

    let (status, body) = post_hook(true, "/v1/hooks/missing", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A web page can't call it (Origin), and without the feature there is no route target.
    let (status, _) = post_hook(true, "/v1/hooks/h1", &[("origin", "https://evil.test")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = post_hook(false, "/v1/hooks/h1", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}
