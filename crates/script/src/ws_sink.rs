//! [`WsSink`]: send browser commands as JSON-RPC 2.0 over a WebSocket.
//!
//! Talks to the browser's `ws://127.0.0.1:47800/v1/ws` (see `crates/api/README.md`);
//! checked against the real browser with `scripts/examples/hello.ts`. Protocol:
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":null}
//! <- {"jsonrpc":"2.0","id":1,"result":{"tabs":[...]}}
//! <- {"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"...","data":{"code":"TAB_NOT_FOUND",...}}}
//! <- {"jsonrpc":"2.0","method":"tab.created","params":{...}}   (event, see below)
//! ```
//!
//! Design: one background task owns the socket. `call` sends the request to
//! that task through a channel together with a "reply slot" (a oneshot
//! channel); the task matches responses to requests by `id` and fills the slot.
//! Many calls can be in flight at once (e.g. `Promise.all` in a script).
//!
//! Events (`browser.on`): notifications (a `method` and no `id`) are dropped until the script
//! first asks for one with [`CommandSink::next_event`]; from then on the socket task queues
//! them (at most [`EVENT_QUEUE`], newer ones are dropped when the script falls behind). So a
//! script that never calls `browser.on` never buffers the default event stream.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use deno_core::futures::{SinkExt, StreamExt};
use deno_core::serde_json::{self, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::sink::{CallFuture, CommandSink, EventFuture, SinkError};

/// Default address of the browser's WebSocket API.
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:47800/v1/ws";

/// How many events may wait for the script before newer ones are dropped.
pub const EVENT_QUEUE: usize = 1024;

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
    /// Set by the first `next_event`: from then on the socket task queues events.
    events_wanted: Arc<AtomicBool>,
    /// Events queued by the socket task. A tokio `Mutex` because `next_event` holds it across
    /// an `.await` (a `RefCell` borrow would panic if JS asked twice at once).
    events: Rc<tokio::sync::Mutex<mpsc::Receiver<(String, Value)>>>,
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
        let (events_tx, events_rx) = mpsc::channel::<(String, Value)>(EVENT_QUEUE);
        let events_wanted = Arc::new(AtomicBool::new(false));
        let wanted = events_wanted.clone();

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
                            Some(Ok(Message::Text(text))) => {
                                handle_message(text.as_str(), &mut waiting, &wanted, &events_tx)
                            }
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
            // `events_tx` is dropped here, so `next_event` returns `None` from now on.
        });

        Ok(Self {
            to_socket,
            next_id: Cell::new(1),
            events_wanted,
            events: Rc::new(tokio::sync::Mutex::new(events_rx)),
        })
    }
}

/// Parse one incoming message: a JSON-RPC response completes the matching call, an event
/// notification is queued for `browser.on` (once the script wants events).
fn handle_message(
    text: &str,
    waiting: &mut HashMap<u64, Reply>,
    events_wanted: &AtomicBool,
    events: &mpsc::Sender<(String, Value)>,
) {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return; // not JSON: ignore
    };
    // Messages without an id are notifications (events).
    let Some(id) = msg.get("id").and_then(Value::as_u64) else {
        if let Some(topic) = msg.get("method").and_then(Value::as_str)
            && events_wanted.load(Ordering::Relaxed)
        {
            let data = msg.get("params").cloned().unwrap_or(Value::Null);
            // Full queue = the script is too slow: drop this event rather than block replies.
            let _ = events.try_send((topic.to_string(), data));
        }
        return;
    };
    let Some(reply) = waiting.remove(&id) else {
        return;
    };
    let result = if let Some(err) = msg.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown browser error");
        // The browser puts its own error code ("TAB_NOT_FOUND", …) in `data.code`.
        let message = match err.pointer("/data/code").and_then(Value::as_str) {
            Some(code) => format!("{code}: {message}"),
            None => message.to_string(),
        };
        Err(SinkError {
            code: err.get("code").and_then(Value::as_i64),
            message,
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

    fn next_event(&self) -> EventFuture {
        self.events_wanted.store(true, Ordering::Relaxed);
        let events = self.events.clone();
        Box::pin(async move { events.lock().await.recv().await })
    }
}
