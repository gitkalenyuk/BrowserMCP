//! Automatic history recording from the [`EventBus`].
//!
//! [`spawn_history_recorder`] starts a task that watches two kinds of events:
//!
//! * `tab.updated {id, title?, url?, loading?, …}` (from the CEF layer; fields may be partial).
//!   We keep the last known url/title/loading per tab and record a visit when a tab with a
//!   non-empty URL is *not loading* and it either just finished loading or its URL changed.
//!   A later title-only update fixes the stored title.
//! * `command.started` / `command.finished` of `nav.goto` — the fallback until tab events exist.
//!   `command.finished` carries no URL, so we remember the URL from `command.started` (same
//!   `call_id`) and, on success, ask the backend for the tab's final URL and title.
//!
//! Both paths may see the same page load; [`history::record_visit`]'s 30-second dedupe makes
//! that one visit.

use std::collections::HashMap;
use std::sync::Arc;

use bmcp_core::events::topics;
use bmcp_core::{BrowserBackend, Event, EventBus};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::history;
use crate::store::{DataResult, DataStore};

/// Topic of tab state changes (emitted by the CEF layer).
pub const TAB_UPDATED: &str = "tab.updated";
/// Topic of a closed tab.
pub const TAB_CLOSED: &str = "tab.closed";

/// What we last saw of one tab.
#[derive(Debug, Default)]
struct TabSeen {
    url: String,
    title: String,
    loading: bool,
}

/// A `nav.goto` call that started and has not finished yet.
#[derive(Debug)]
struct PendingGoto {
    url: String,
    /// Explicit target tab, `None` = the default (highest id).
    tab: Option<i64>,
}

/// Turns events into history visits. Use [`spawn_history_recorder`] in the app; tests feed
/// events to [`HistoryRecorder::handle`] directly.
pub struct HistoryRecorder {
    store: DataStore,
    /// Used to look up the final URL/title after `nav.goto`; `None` = use the requested URL.
    backend: Option<Arc<dyn BrowserBackend>>,
    tabs: HashMap<i64, TabSeen>,
    pending_goto: HashMap<u64, PendingGoto>,
}

impl HistoryRecorder {
    pub fn new(store: DataStore, backend: Option<Arc<dyn BrowserBackend>>) -> Self {
        HistoryRecorder {
            store,
            backend,
            tabs: HashMap::new(),
            pending_goto: HashMap::new(),
        }
    }

    /// Process one event (records at most one visit).
    pub async fn handle(&mut self, event: &Event) -> DataResult<()> {
        let data = &event.data;
        match event.topic.as_str() {
            TAB_UPDATED => self.on_tab_updated(data, event.ts_ms as i64).await,
            TAB_CLOSED => {
                if let Some(id) = data["id"].as_i64() {
                    self.tabs.remove(&id);
                }
                Ok(())
            }
            topics::COMMAND_STARTED if data["name"] == "nav.goto" => {
                if let (Some(call_id), Some(url)) =
                    (data["call_id"].as_u64(), data["args"]["url"].as_str())
                {
                    let tab = data["tab"].as_i64();
                    self.pending_goto.insert(
                        call_id,
                        PendingGoto {
                            url: url.to_owned(),
                            tab,
                        },
                    );
                }
                Ok(())
            }
            topics::COMMAND_FINISHED if data["name"] == "nav.goto" => {
                let Some(pending) = data["call_id"]
                    .as_u64()
                    .and_then(|id| self.pending_goto.remove(&id))
                else {
                    return Ok(());
                };
                if data["ok"] != Value::Bool(true) {
                    return Ok(());
                }
                let (url, title) = self.final_page(&pending).await;
                self.record(url, title, "typed", event.ts_ms as i64).await
            }
            _ => Ok(()),
        }
    }

    async fn on_tab_updated(&mut self, data: &Value, time: i64) -> DataResult<()> {
        let Some(id) = data["id"].as_i64() else {
            return Ok(());
        };
        let seen = self.tabs.entry(id).or_default();
        let (was_loading, old_url, old_title) =
            (seen.loading, seen.url.clone(), seen.title.clone());
        if let Some(url) = data["url"].as_str() {
            seen.url = url.to_owned();
        }
        if let Some(title) = data["title"].as_str() {
            seen.title = title.to_owned();
        }
        if let Some(loading) = data["loading"].as_bool() {
            seen.loading = loading;
        }
        if seen.loading || seen.url.is_empty() {
            return Ok(());
        }
        let (url, title) = (seen.url.clone(), seen.title.clone());
        if was_loading || url != old_url {
            self.record(url, title, "link", time).await
        } else if title != old_title {
            self.store
                .run(move |c| history::update_title(c, &url, &title))
                .await
        } else {
            Ok(())
        }
    }

    /// URL and title of the tab after a successful `nav.goto` (redirects included).
    async fn final_page(&self, pending: &PendingGoto) -> (String, String) {
        let fallback = (pending.url.clone(), String::new());
        let Some(backend) = &self.backend else {
            return fallback;
        };
        let Ok(tabs) = backend.list_tabs().await else {
            return fallback;
        };
        let tab = match pending.tab {
            Some(id) => tabs.into_iter().find(|t| i64::from(t.id.0) == id),
            None => tabs.into_iter().max_by_key(|t| t.id),
        };
        match tab {
            Some(tab) if !tab.url.is_empty() => (tab.url, tab.title),
            _ => fallback,
        }
    }

    async fn record(
        &self,
        url: String,
        title: String,
        transition: &'static str,
        time: i64,
    ) -> DataResult<()> {
        self.store
            .run(move |c| history::record_visit(c, &url, &title, transition, time))
            .await
            .map(|_recorded| ())
    }
}

/// Start recording history from `events` on the current tokio runtime. The subscription is
/// made before this returns, so no later event is missed. `backend` is used to read the final
/// URL and title after `nav.goto`.
pub fn spawn_history_recorder(
    store: DataStore,
    events: &EventBus,
    backend: Option<Arc<dyn BrowserBackend>>,
) -> JoinHandle<()> {
    let mut rx = events.subscribe();
    let mut recorder = HistoryRecorder::new(store, backend);
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(err) = recorder.handle(&event).await {
                        tracing::warn!(%err, topic = %event.topic, "history recorder");
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(
                        missed,
                        "history recorder fell behind; some visits were lost"
                    );
                }
                Err(RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bmcp_core::testing::{FakeBackend, ctx};
    use bmcp_core::{Caller, Registry, TabId};
    use serde_json::json;

    use super::*;

    fn event(topic: &str, ts_ms: u64, data: Value) -> Event {
        Event {
            seq: 0,
            topic: topic.into(),
            ts_ms,
            data,
        }
    }

    fn all(store: &DataStore) -> Vec<history::HistoryEntry> {
        store.with(|c| history::search(c, None, None, 100)).unwrap()
    }

    #[tokio::test]
    async fn tab_updated_records_when_loading_ends() {
        let store = DataStore::open_in_memory().unwrap();
        let mut rec = HistoryRecorder::new(store.clone(), None);
        let t0 = 1_000_000;

        // Loading starts: nothing yet.
        rec.handle(&event(
            TAB_UPDATED,
            t0,
            json!({"id": 1, "url": "https://a.test/", "title": "", "loading": true}),
        ))
        .await
        .unwrap();
        assert!(all(&store).is_empty());
        // Loading ends (partial payload): one visit.
        rec.handle(&event(
            TAB_UPDATED,
            t0 + 500,
            json!({"id": 1, "loading": false}),
        ))
        .await
        .unwrap();
        // The title arrives later and is stored.
        rec.handle(&event(
            TAB_UPDATED,
            t0 + 600,
            json!({"id": 1, "title": "Page A"}),
        ))
        .await
        .unwrap();
        let entries = all(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Page A");
        assert_eq!(entries[0].visit_count, 1);
        assert_eq!(entries[0].last_visit, (t0 + 500) as i64);

        // A reload within 30 s is the same visit.
        rec.handle(&event(
            TAB_UPDATED,
            t0 + 5_000,
            json!({"id": 1, "loading": true}),
        ))
        .await
        .unwrap();
        rec.handle(&event(
            TAB_UPDATED,
            t0 + 6_000,
            json!({"id": 1, "loading": false}),
        ))
        .await
        .unwrap();
        assert_eq!(all(&store)[0].visit_count, 1);
        // After 30 s it counts again.
        rec.handle(&event(
            TAB_UPDATED,
            t0 + 40_000,
            json!({"id": 1, "loading": true}),
        ))
        .await
        .unwrap();
        rec.handle(&event(
            TAB_UPDATED,
            t0 + 41_000,
            json!({"id": 1, "loading": false}),
        ))
        .await
        .unwrap();
        assert_eq!(all(&store)[0].visit_count, 2);

        // Empty URL or internal pages are not recorded.
        rec.handle(&event(
            TAB_UPDATED,
            t0,
            json!({"id": 2, "url": "", "loading": false}),
        ))
        .await
        .unwrap();
        rec.handle(&event(
            TAB_UPDATED,
            t0,
            json!({"id": 3, "url": "bmcp://ui/newtab", "loading": false}),
        ))
        .await
        .unwrap();
        assert_eq!(all(&store).len(), 1);

        rec.handle(&event(TAB_CLOSED, t0, json!({"id": 1})))
            .await
            .unwrap();
        assert!(!rec.tabs.contains_key(&1));
    }

    #[tokio::test]
    async fn nav_goto_through_the_registry_is_recorded() {
        // The fake's navigate sets title = url, like a page without <title>.
        let fake = FakeBackend::with_tabs(&["https://start.test/"]);
        let store = DataStore::open_in_memory().unwrap();
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        let ctx = ctx(Caller::Rest, fake.clone());
        let task = spawn_history_recorder(store.clone(), &ctx.events, Some(fake.clone()));

        registry
            .call(ctx.clone(), "nav.goto", json!({"url": "https://b.test/x"}))
            .await
            .unwrap();
        // A failed navigation records nothing.
        let _ = registry
            .call(
                ctx.clone(),
                "nav.goto",
                json!({"url": "https://c.test/", "tab": 99}),
            )
            .await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while all(&store).is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "visit not recorded");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Let the recorder see the failing call too.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let entries = all(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://b.test/x");
        task.abort();
    }

    #[tokio::test]
    async fn nav_goto_uses_the_final_url_from_the_backend() {
        let fake = FakeBackend::with_tabs(&["https://a.test/", "https://b.test/"]);
        let store = DataStore::open_in_memory().unwrap();
        let mut rec = HistoryRecorder::new(store.clone(), Some(fake.clone()));
        // Pretend tab 1 was redirected to /final with a title.
        fake.set_tab(TabId(1), "Final", "https://a.test/final", false);
        rec.handle(&event(
            topics::COMMAND_STARTED,
            1,
            json!({"call_id": 7, "name": "nav.goto", "args": {"url": "https://a.test/"}, "tab": 1}),
        ))
        .await
        .unwrap();
        rec.handle(&event(
            topics::COMMAND_FINISHED,
            2_000,
            json!({"call_id": 7, "name": "nav.goto", "ok": true}),
        ))
        .await
        .unwrap();
        let entries = all(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].url.as_str(), entries[0].title.as_str()),
            ("https://a.test/final", "Final")
        );
        assert!(rec.pending_goto.is_empty());

        // Without a backend the requested URL is used.
        let mut rec = HistoryRecorder::new(store.clone(), None);
        rec.handle(&event(topics::COMMAND_STARTED, 1, json!({"call_id": 8, "name": "nav.goto", "args": {"url": "https://z.test/"}, "tab": null})))
            .await
            .unwrap();
        rec.handle(&event(
            topics::COMMAND_FINISHED,
            2_000,
            json!({"call_id": 8, "name": "nav.goto", "ok": true}),
        ))
        .await
        .unwrap();
        assert!(all(&store).iter().any(|e| e.url == "https://z.test/"));
    }
}
