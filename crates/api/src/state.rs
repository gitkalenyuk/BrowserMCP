//! Shared state of the REST and WebSocket handlers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bmcp_core::{BrowserBackend, Caller, Ctx, EventBus, Registry};
use tokio_util::sync::CancellationToken;

/// Everything a REST/WS handler needs. Cheap to clone (all fields are shared handles).
#[derive(Clone)]
pub struct ApiState {
    pub registry: Arc<Registry>,
    pub backend: Arc<dyn BrowserBackend>,
    pub events: EventBus,
    /// Cancelled when the browser shuts down; open WebSocket connections close on it.
    pub shutdown: CancellationToken,
    /// Source of `Caller::Ws { conn }` numbers (1, 2, 3…).
    next_conn: Arc<AtomicU64>,
}

impl ApiState {
    pub fn new(
        registry: Arc<Registry>,
        backend: Arc<dyn BrowserBackend>,
        events: EventBus,
        shutdown: CancellationToken,
    ) -> Self {
        ApiState {
            registry,
            backend,
            events,
            shutdown,
            next_conn: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The context for one command call made by `caller`.
    /// (Same construction as in `mcp.rs`; keep the two in sync.)
    pub fn ctx(&self, caller: Caller) -> Ctx {
        Ctx::new(caller, self.backend.clone(), self.events.clone())
    }

    /// A new WebSocket connection number.
    pub fn next_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed) + 1
    }
}
