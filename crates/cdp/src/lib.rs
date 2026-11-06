//! Minimal Chrome DevTools Protocol client.
//!
//! The client only does JSON-RPC bookkeeping: it assigns ids, matches responses to
//! callers and fans out events. How bytes travel is the [`Transport`]'s job — inside
//! BrowserMCP that is CEF's in-process `SendDevToolsMessage` (no debugging port, so
//! pages cannot detect automation through it); tests use an in-memory transport.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{broadcast, oneshot};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Default time a single CDP call may take.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum CdpError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("CDP error {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("CDP call {method} timed out")]
    Timeout { method: String },
    #[error("CDP connection closed")]
    Closed,
}

/// Sends one serialized CDP message. Resolves once the message was handed to the
/// browser (not when the answer arrives).
pub trait Transport: Send + Sync + 'static {
    fn send(&self, message: String) -> BoxFuture<'static, Result<(), CdpError>>;
}

/// CDP events are the core type so `BrowserBackend::cdp_events` can hand them out unchanged.
pub use bmcp_core::CdpEvent;

type Pending = HashMap<u64, oneshot::Sender<Result<Value, CdpError>>>;

pub struct CdpClient {
    transport: Box<dyn Transport>,
    next_id: AtomicU64,
    pending: Mutex<Pending>,
    events: broadcast::Sender<CdpEvent>,
    timeout: Duration,
}

impl CdpClient {
    pub fn new(transport: impl Transport) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            transport: Box::new(transport),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            events,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Call a CDP method on the page target and wait for its result.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.call_in_session(None, method, params).await
    }

    /// Call a CDP method, optionally inside a flattened child session (iframes, workers).
    pub async fn call_in_session(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session_id {
            message["sessionId"] = Value::String(session.to_owned());
        }

        let (tx, rx) = oneshot::channel();
        self.pending_map().insert(id, tx);

        if let Err(err) = self.transport.send(message.to_string()).await {
            self.pending_map().remove(&id);
            return Err(err);
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CdpError::Closed),
            Err(_) => {
                self.pending_map().remove(&id);
                Err(CdpError::Timeout {
                    method: method.to_owned(),
                })
            }
        }
    }

    /// Subscribe to all CDP events from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Feed one raw message received from the browser.
    pub fn handle_message(&self, raw: &[u8]) {
        let value: Value = match serde_json::from_slice(raw) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(%err, "undecodable CDP message");
                return;
            }
        };

        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            let Some(tx) = self.pending_map().remove(&id) else {
                return; // Caller timed out or the id belongs to someone else.
            };
            let result = match value.get("error") {
                Some(error) => Err(CdpError::Protocol {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_owned(),
                }),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(result);
        } else if let Some(method) = value.get("method").and_then(Value::as_str) {
            // No receivers is fine: nobody is listening for events right now.
            let _ = self.events.send(CdpEvent {
                method: method.to_owned(),
                params: value.get("params").cloned().unwrap_or(Value::Null),
                session_id: value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }

    /// The browser behind this client went away: fail every in-flight call.
    pub fn close(&self) {
        for (_, tx) in self.pending_map().drain() {
            let _ = tx.send(Err(CdpError::Closed));
        }
    }

    fn pending_map(&self) -> std::sync::MutexGuard<'_, Pending> {
        // A panic while holding this lock cannot leave the map inconsistent.
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// Transport that hands every sent message to the test.
    struct Loopback(mpsc::UnboundedSender<Value>);

    impl Transport for Loopback {
        fn send(&self, message: String) -> BoxFuture<'static, Result<(), CdpError>> {
            let sent = self.0.send(serde_json::from_str(&message).unwrap());
            Box::pin(async move { sent.map_err(|_| CdpError::Closed) })
        }
    }

    struct Broken;
    impl Transport for Broken {
        fn send(&self, _: String) -> BoxFuture<'static, Result<(), CdpError>> {
            Box::pin(async { Err(CdpError::Transport("tab gone".into())) })
        }
    }

    fn client() -> (Arc<CdpClient>, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Arc::new(CdpClient::new(Loopback(tx))), rx)
    }

    #[tokio::test]
    async fn result_is_matched_by_id() {
        let (client, mut sent) = client();
        let c = client.clone();
        let call = tokio::spawn(async move {
            c.call("Page.navigate", serde_json::json!({"url": "x"}))
                .await
        });

        let msg = sent.recv().await.unwrap();
        assert_eq!(msg["method"], "Page.navigate");
        assert_eq!(msg["params"]["url"], "x");
        let id = msg["id"].as_u64().unwrap();

        client.handle_message(format!(r#"{{"id":{id},"result":{{"frameId":"F"}}}}"#).as_bytes());
        assert_eq!(call.await.unwrap().unwrap()["frameId"], "F");
    }

    #[tokio::test]
    async fn protocol_errors_are_reported() {
        let (client, mut sent) = client();
        let c = client.clone();
        let call = tokio::spawn(async move { c.call("Bogus.method", Value::Null).await });
        let id = sent.recv().await.unwrap()["id"].as_u64().unwrap();
        client.handle_message(
            format!(r#"{{"id":{id},"error":{{"code":-32601,"message":"not found"}}}}"#).as_bytes(),
        );
        assert_eq!(
            call.await.unwrap(),
            Err(CdpError::Protocol {
                code: -32601,
                message: "not found".into()
            })
        );
    }

    #[tokio::test]
    async fn events_are_broadcast_with_session() {
        let (client, _sent) = client();
        let mut events = client.subscribe();
        client.handle_message(
            br#"{"method":"Page.loadEventFired","params":{"timestamp":1},"sessionId":"S1"}"#,
        );
        let ev = events.recv().await.unwrap();
        assert_eq!(ev.method, "Page.loadEventFired");
        assert_eq!(ev.session_id.as_deref(), Some("S1"));
    }

    #[tokio::test]
    async fn session_id_is_sent() {
        let (client, mut sent) = client();
        let c = client.clone();
        tokio::spawn(async move {
            c.call_in_session(Some("S9"), "DOM.enable", Value::Null)
                .await
        });
        assert_eq!(sent.recv().await.unwrap()["sessionId"], "S9");
    }

    #[tokio::test]
    async fn timeout_and_close() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = Arc::new(CdpClient::new(Loopback(tx)).with_timeout(Duration::from_millis(20)));
        assert!(matches!(
            client.call("Slow.method", Value::Null).await,
            Err(CdpError::Timeout { .. })
        ));
        assert!(
            client.pending_map().is_empty(),
            "timed-out call must be forgotten"
        );

        let c = client.clone();
        let call =
            tokio::spawn(async move { CdpClient::call(&c, "Never.answers", Value::Null).await });
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        client.close();
        assert_eq!(call.await.unwrap(), Err(CdpError::Closed));
    }

    #[tokio::test]
    async fn transport_failure_is_returned() {
        let client = CdpClient::new(Broken);
        assert_eq!(
            client.call("X.y", Value::Null).await,
            Err(CdpError::Transport("tab gone".into()))
        );
        assert!(client.pending_map().is_empty());
    }

    #[test]
    fn garbage_and_unknown_ids_are_ignored() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = CdpClient::new(Loopback(tx));
        client.handle_message(b"not json");
        client.handle_message(br#"{"id":999,"result":{}}"#);
    }
}
