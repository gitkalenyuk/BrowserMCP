//! BrowserMCP API servers.
//!
//! Security model (local only, no tokens — user's choice):
//! * bind to 127.0.0.1 only;
//! * `Host` must be a loopback name (rmcp default) — blocks DNS rebinding;
//! * any request carrying an `Origin` header is rejected — web pages, including the ones
//!   open in BrowserMCP itself, always send `Origin`; MCP clients like Claude Code don't.

mod ax;
mod mcp;

use std::net::SocketAddr;
use std::sync::Arc;

use bmcp_core::BrowserBackend;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

pub use ax::format_ax_tree;
pub use mcp::BrowserMcp;

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
}

/// Start the HTTP server (MCP at `/mcp`) on 127.0.0.1:`port` (0 = any free port).
pub async fn serve(
    backend: Arc<dyn BrowserBackend>,
    port: u16,
    shutdown: CancellationToken,
) -> Result<Server, ServeError> {
    let config = StreamableHttpServerConfig::default()
        .enforce_origin_validation()
        .with_cancellation_token(shutdown.child_token());
    let mcp = StreamableHttpService::new(
        move || Ok(BrowserMcp::new(backend.clone())),
        LocalSessionManager::default().into(),
        config,
    );
    let router = axum::Router::new().nest_service("/mcp", mcp);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| ServeError::Bind { addr, source })?;
    let addr = listener.local_addr().unwrap_or(addr);

    let task = tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await;
        if let Err(err) = result {
            tracing::error!(%err, "API server stopped");
        }
    });
    tracing::info!(%addr, "MCP server listening at http://{addr}/mcp");
    Ok(Server { addr, task })
}
