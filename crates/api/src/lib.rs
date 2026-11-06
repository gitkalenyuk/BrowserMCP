//! BrowserMCP API servers, all on one port (127.0.0.1:47800 by default):
//!
//! * `/mcp` — MCP over Streamable HTTP (`mcp.rs`);
//! * `/v1/health`, `/v1/commands`, `/v1/cmd/{name}`, `/openapi.json` — REST ([`rest`]);
//! * `/v1/ws` — JSON-RPC 2.0 over WebSocket + event notifications ([`ws`]).
//!
//! Every surface is generated from the same command [`Registry`]. See `crates/api/README.md`.
//!
//! Security model (local only, no tokens — user's choice):
//! * bind to 127.0.0.1 only;
//! * `Host` must be a loopback name — blocks DNS rebinding (rmcp checks it for `/mcp`,
//!   [`guard`] for everything else);
//! * `/mcp` rejects any request carrying an `Origin` header — web pages, including the ones
//!   open in BrowserMCP itself, always send `Origin`; MCP clients like Claude Code don't;
//! * REST/WS reject `Origin` too, except the browser's own UI (`bmcp://ui`, plus the Vite dev
//!   server when `BMCP_DEV_UI=1`), see [`guard`].

mod ax;
pub mod commands;
pub mod guard;
mod mcp;
pub mod openapi;
pub mod rest;
mod state;
pub mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use bmcp_core::{BrowserBackend, EventBus, Registry};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

pub use ax::format_ax_tree;
pub use guard::OriginPolicy;
pub use mcp::{BrowserMcp, DEFAULT_TOOLSETS};
pub use state::ApiState;

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

/// A running API server.
pub struct Server {
    pub addr: SocketAddr,
    pub task: tokio::task::JoinHandle<()>,
}

impl Server {
    pub fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Base of the REST API, e.g. `http://127.0.0.1:47800/v1`.
    pub fn rest_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/v1/ws", self.addr)
    }
}

/// The REST + WebSocket routes (everything except `/mcp`), behind the Host/Origin guard.
/// [`serve`] mounts it next to MCP; tests call it directly with `tower::ServiceExt::oneshot`.
pub fn api_router(state: ApiState, policy: OriginPolicy) -> axum::Router {
    rest::router()
        .merge(ws::router())
        .layer(axum::middleware::from_fn_with_state(policy, guard::guard))
        .with_state(state)
}

/// Start the HTTP server (MCP at `/mcp`, REST + WS at `/v1`) on 127.0.0.1:`port`
/// (0 = any free port). Dev mode of the UI origin check comes from `BMCP_DEV_UI`.
pub async fn serve(
    backend: Arc<dyn BrowserBackend>,
    registry: Arc<Registry>,
    events: EventBus,
    port: u16,
    shutdown: CancellationToken,
) -> Result<Server, ServeError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener =
        std::net::TcpListener::bind(addr).map_err(|source| ServeError::Bind { addr, source })?;
    serve_on(backend, registry, events, listener, shutdown).await
}

/// Like [`serve`], on a socket the caller already bound. The app binds before starting CEF so
/// the port it tells the UI pages and AI agents is guaranteed to be ours.
pub async fn serve_on(
    backend: Arc<dyn BrowserBackend>,
    registry: Arc<Registry>,
    events: EventBus,
    listener: std::net::TcpListener,
    shutdown: CancellationToken,
) -> Result<Server, ServeError> {
    let api = api_router(
        ApiState::new(
            registry.clone(),
            backend.clone(),
            events.clone(),
            shutdown.clone(),
        ),
        OriginPolicy::from_env(),
    );

    let config = StreamableHttpServerConfig::default()
        .enforce_origin_validation()
        .with_cancellation_token(shutdown.child_token());
    let mcp = StreamableHttpService::new(
        move || {
            Ok(BrowserMcp::new(
                registry.clone(),
                backend.clone(),
                events.clone(),
            ))
        },
        LocalSessionManager::default().into(),
        config,
    );
    let router = axum::Router::new().nest_service("/mcp", mcp).merge(api);

    let fallback = SocketAddr::from(([127, 0, 0, 1], 0));
    let addr = listener.local_addr().unwrap_or(fallback);
    let listener = listener
        .set_nonblocking(true)
        .and_then(|()| tokio::net::TcpListener::from_std(listener))
        .map_err(|source| ServeError::Bind { addr, source })?;

    let task = tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await;
        if let Err(err) = result {
            tracing::error!(%err, "API server stopped");
        }
    });
    tracing::info!(%addr, "MCP server listening at http://{addr}/mcp");
    tracing::info!("REST at http://{addr}/v1, WebSocket at ws://{addr}/v1/ws");
    Ok(Server { addr, task })
}
