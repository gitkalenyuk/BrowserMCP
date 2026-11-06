//! [`WsSink`]: send browser commands as JSON-RPC 2.0 over a WebSocket.
//!
//! STATUS: compiles, NOT TESTED. The browser endpoint
//! `ws://127.0.0.1:47800/v1/ws` does not exist yet (phase 3). Assumed protocol:
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":null}
//! <- {"jsonrpc":"2.0","id":1,"result":[...]}
//! <- {"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"..."}}
//! ```
//!
//! Design: one background task owns the socket. `call` sends the request to
//! that task through a channel together with a "reply slot" (a oneshot
//! channel); the task matches responses to requests by `id` and fills the slot.
//! Many calls can be in flight at once (e.g. `Promise.all` in a script).

use std::cell::Cell;
use std::collections::HashMap;

use deno_core::futures::{SinkExt, StreamExt};
use deno_core::serde_json::{self, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::sink::{CallFuture, CommandSink, SinkError};

/// Default address of the browser's WebSocket API (planned, not built yet).
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:47800/v1/ws";

type Reply = oneshot::Sender<Result<Value, SinkError>>;

/// One request waiting to be written to the socket.
struct Outgoing {
    id: u64,
    text: String,
    reply: Reply,
}

pub struct WsSink {
    to_socket: mpsc::UnboundedSender<Outgoing>,
    /// Next JSON-RPC id. `Cell` is enough: the sink is used from one thread.
    next_id: Cell<u64>,
}

impl WsSink {
    /// Connect to `url` and start the socket task.
    /// Must be called from inside a tokio runtime.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let (socket, _response) = tokio_tungstenite::connect_async(url)
            .await
            // tungstenite's error text already includes its cause, so we
            // format it once here instead of chaining it with `.context()`.
            .map_err(|e| anyhow::anyhow!("cannot connect to browser at {url}: {e}"))?;
        let (mut write, mut read) = socket.split();
        let (to_socket, mut from_calls) = mpsc::unbounded_channel::<Outgoing>();

        tokio::spawn(async move {
            // Requests sent but not answered yet, by id.
            let mut waiting: HashMap<u64, Reply> = HashMap::new();
            loop {
                tokio::select! {
                    outgoing = from_calls.recv() => {
                        // `None` = WsSink was dropped: stop the task.
                        let Some(out) = outgoing else { break };
                        waiting.insert(out.id, out.reply);
                        if write.send(Message::text(out.text)).await.is_err() {
                            break;
                        }
                    }
                    incoming = read.next() => {
                        match incoming {
                            Some(Ok(Message::Text(text))) => handle_response(text.as_str(), &mut waiting),
                            Some(Ok(_)) => {} // ping/pong/binary: ignore
                            _ => break,       // closed or error
                        }
                    }
                }
            }
            // Fail everything still waiting so scripts don't hang forever.
            for (_, reply) in waiting.drain() {
                let _ = reply.send(Err(SinkError::new("browser connection closed")));
            }
        });

        Ok(Self {
            to_socket,
            next_id: Cell::new(1),
        })
    }
}

/// Parse one JSON-RPC response and complete the matching call.
fn handle_response(text: &str, waiting: &mut HashMap<u64, Reply>) {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return; // not JSON: ignore
    };
    // Messages without an id are notifications (events): not handled yet.
    let Some(id) = msg.get("id").and_then(Value::as_u64) else {
        return;
    };
    let Some(reply) = waiting.remove(&id) else {
        return;
    };
    let result = if let Some(err) = msg.get("error") {
        Err(SinkError {
            code: err.get("code").and_then(Value::as_i64),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown browser error")
                .to_string(),
        })
    } else {
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    };
    let _ = reply.send(result);
}

impl CommandSink for WsSink {
    fn call(&self, name: String, args: Value) -> CallFuture {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let text =
            json!({ "jsonrpc": "2.0", "id": id, "method": name, "params": args }).to_string();
        let (reply, answer) = oneshot::channel();
        let sent = self.to_socket.send(Outgoing { id, text, reply });
        Box::pin(async move {
            if sent.is_err() {
                return Err(SinkError::new("browser connection closed"));
            }
            answer
                .await
                .unwrap_or_else(|_| Err(SinkError::new("browser connection closed")))
        })
    }
}
