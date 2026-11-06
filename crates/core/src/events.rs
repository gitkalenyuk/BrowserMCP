//! `EventBus`: an in-process broadcast channel of [`Event`]s.
//!
//! Anyone can [`subscribe`](EventBus::subscribe) (the recorder, the WS server pushing events to
//! clients, the UI). Every event gets a `seq` number that grows by one per event, so a consumer
//! can detect gaps: a slow subscriber that falls more than [`EventBus::CAPACITY`] events behind
//! gets `RecvError::Lagged` from tokio and should resync.
//!
//! Cloning an `EventBus` is cheap and every clone talks to the same channel.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// Well-known event topics.
pub mod topics {
    /// A command passed validation and the lock check and is about to run.
    pub const COMMAND_STARTED: &str = "command.started";
    /// A command that was started has finished (successfully or not).
    pub const COMMAND_FINISHED: &str = "command.finished";
    /// A call was refused before running: unknown command, invalid arguments or tab locked.
    pub const COMMAND_REJECTED: &str = "command.rejected";
}

/// One event on the bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Strictly increasing per bus, starting at 1.
    pub seq: u64,
    /// Dotted topic, e.g. `"command.finished"`; see [`topics`].
    pub topic: String,
    /// Wall-clock time in milliseconds since the Unix epoch.
    pub ts_ms: u64,
    /// Topic-specific payload.
    pub data: Value,
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    /// Last used seq. The lock is held while sending so events enter the channel in seq order
    /// even when several threads emit at the same time.
    last_seq: Arc<Mutex<u64>>,
}

impl EventBus {
    /// How many events a subscriber may fall behind before it starts losing events.
    pub const CAPACITY: usize = 1024;

    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(Self::CAPACITY);
        EventBus {
            sender,
            last_seq: Arc::new(Mutex::new(0)),
        }
    }

    /// Publish an event and return its `seq`. Never blocks; having no subscribers is fine.
    pub fn emit(&self, topic: impl Into<String>, data: Value) -> u64 {
        // A poisoned lock only means another thread panicked mid-emit; the counter is still valid.
        let mut last_seq = self.last_seq.lock().unwrap_or_else(|e| e.into_inner());
        *last_seq += 1;
        let event = Event {
            seq: *last_seq,
            topic: topic.into(),
            ts_ms: now_ms(),
            data,
        };
        // `send` only fails when nobody is subscribed, which is not an error for us.
        let _ = self.sender.send(event);
        *last_seq
    }

    /// Receive every event emitted from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("subscribers", &self.sender.receiver_count())
            .finish()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn seq_is_strictly_increasing_across_threads() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let bus = bus.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        bus.emit("test", json!(null));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        for expected in 1..=200 {
            assert_eq!(rx.recv().await.unwrap().seq, expected);
        }
    }

    #[test]
    fn emit_without_subscribers_is_fine() {
        let bus = EventBus::new();
        assert_eq!(bus.emit("a", json!(1)), 1);
        assert_eq!(bus.emit("b", json!(2)), 2);
    }
}
