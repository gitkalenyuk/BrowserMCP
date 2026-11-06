//! Test helpers: an in-memory [`FakeBackend`].
//!
//! Compiled for this crate's tests, and for other crates with the `testing` feature:
//! `bmcp-core = { workspace = true, features = ["testing"] }` under `[dev-dependencies]`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::backend::{
    BackendError, BackendResult, BoxFuture, BrowserBackend, CdpEvent, CdpEvents, TabId, TabInfo,
};
use crate::command::{Caller, Ctx};
use crate::events::EventBus;

/// A browser that keeps tabs in a `Vec`. Navigation finishes instantly; the page title becomes
/// the URL; `Runtime.evaluate` returns the text set with [`FakeBackend::set_text`].
///
/// Other CDP methods can be scripted with [`FakeBackend::on_cdp`]; every CDP call is logged with
/// its parameters ([`FakeBackend::cdp_calls`]).
#[derive(Default)]
pub struct FakeBackend {
    state: Mutex<FakeState>,
}

/// Scripted CDP answers: `(tab, method, params)` → `Some(result)` to answer, `None` to fall back
/// to the built-in behaviour.
pub type CdpHandler = Box<dyn Fn(TabId, &str, &Value) -> Option<BackendResult<Value>> + Send>;

#[derive(Default)]
struct FakeState {
    tabs: Vec<TabInfo>,
    texts: HashMap<TabId, String>,
    /// Returned (once) by the next backend call instead of doing the work.
    fail_next: Option<BackendError>,
    /// Log like `"navigate 1 https://a.test"`, to assert what the backend was asked to do.
    calls: Vec<String>,
    /// Every CDP call as `(method, params)`, in order.
    cdp_calls: Vec<(String, Value)>,
    cdp_handler: Option<CdpHandler>,
    /// One CDP event channel per tab, created on first subscription or emit.
    cdp_events: HashMap<TabId, tokio::sync::broadcast::Sender<CdpEvent>>,
}

impl FakeBackend {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Backend with one tab per URL (ids 1, 2, …).
    pub fn with_tabs(urls: &[&str]) -> Arc<Self> {
        let fake = Self::default();
        {
            let mut state = fake.lock();
            for url in urls {
                state.add_tab(url.to_string());
            }
            state.calls.clear();
        }
        Arc::new(fake)
    }

    pub fn set_text(&self, tab: TabId, text: &str) {
        self.lock().texts.insert(tab, text.to_string());
    }

    /// Make the next backend call fail with `err`.
    pub fn fail_next(&self, err: BackendError) {
        self.lock().fail_next = Some(err);
    }

    /// Answer CDP calls with `handler` first (see [`CdpHandler`]). Replaces an earlier handler.
    /// The handler runs while the fake's state is locked, so it must not call the fake.
    pub fn on_cdp(
        &self,
        handler: impl Fn(TabId, &str, &Value) -> Option<BackendResult<Value>> + Send + 'static,
    ) {
        self.lock().cdp_handler = Some(Box::new(handler));
    }

    /// Every CDP call so far as `(method, params)`.
    pub fn cdp_calls(&self) -> Vec<(String, Value)> {
        self.lock().cdp_calls.clone()
    }

    /// Deliver a fake CDP event to the tab's [`BrowserBackend::cdp_events`] subscribers.
    /// Returns how many subscribers received it.
    pub fn emit_cdp_event(&self, tab: TabId, method: &str, params: Value) -> usize {
        let sender = self.lock().event_sender(tab);
        sender
            .send(CdpEvent {
                method: method.to_string(),
                params,
                session_id: None,
            })
            .unwrap_or(0)
    }

    /// Change a tab's title, URL and loading flag (as seen by `list_tabs`).
    pub fn set_tab(&self, tab: TabId, title: &str, url: &str, loading: bool) {
        if let Ok(info) = self.lock().tab_mut(tab) {
            info.title = title.to_string();
            info.url = url.to_string();
            info.loading = loading;
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    pub fn tabs(&self) -> Vec<TabInfo> {
        self.lock().tabs.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` on the state unless a failure was injected; return the result as a ready future.
    fn with_state<T: Send + 'static>(
        &self,
        call: String,
        f: impl FnOnce(&mut FakeState) -> BackendResult<T>,
    ) -> BoxFuture<'_, BackendResult<T>> {
        let mut state = self.lock();
        state.calls.push(call);
        let result = match state.fail_next.take() {
            Some(err) => Err(err),
            None => f(&mut state),
        };
        Box::pin(std::future::ready(result))
    }
}

impl FakeState {
    fn add_tab(&mut self, url: String) -> TabId {
        let id = TabId(self.tabs.iter().map(|t| t.id.0).max().unwrap_or(0) + 1);
        self.tabs.push(TabInfo {
            id,
            title: url.clone(),
            url,
            loading: false,
        });
        id
    }

    fn event_sender(&mut self, tab: TabId) -> tokio::sync::broadcast::Sender<CdpEvent> {
        self.cdp_events
            .entry(tab)
            .or_insert_with(|| tokio::sync::broadcast::channel(1024).0)
            .clone()
    }

    fn tab_mut(&mut self, tab: TabId) -> BackendResult<&mut TabInfo> {
        self.tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .ok_or(BackendError::TabNotFound(tab))
    }
}

impl BrowserBackend for FakeBackend {
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>> {
        self.with_state("list_tabs".into(), |s| Ok(s.tabs.clone()))
    }

    fn open_tab(&self, url: String) -> BoxFuture<'_, BackendResult<TabId>> {
        self.with_state(format!("open_tab {url}"), |s| Ok(s.add_tab(url)))
    }

    fn navigate(
        &self,
        tab: TabId,
        url: String,
        _timeout: Duration,
    ) -> BoxFuture<'_, BackendResult<TabInfo>> {
        self.with_state(format!("navigate {tab} {url}"), |s| {
            let info = s.tab_mut(tab)?;
            info.title = url.clone();
            info.url = url;
            Ok(info.clone())
        })
    }

    fn cdp(
        &self,
        tab: TabId,
        method: String,
        params: Value,
    ) -> BoxFuture<'_, BackendResult<Value>> {
        self.with_state(format!("cdp {tab} {method}"), |s| {
            s.tab_mut(tab)?;
            s.cdp_calls.push((method.clone(), params.clone()));
            if let Some(answer) = s
                .cdp_handler
                .as_ref()
                .and_then(|h| h(tab, &method, &params))
            {
                return answer;
            }
            match method.as_str() {
                "Runtime.evaluate" => {
                    let text = s.texts.get(&tab).cloned().unwrap_or_default();
                    Ok(json!({ "result": { "type": "string", "value": text } }))
                }
                _ => Err(BackendError::Cdp(format!("{method} is not faked"))),
            }
        })
    }

    fn cdp_events(&self, tab: TabId) -> BoxFuture<'_, BackendResult<CdpEvents>> {
        self.with_state(format!("cdp_events {tab}"), |s| {
            s.tab_mut(tab)?;
            Ok(s.event_sender(tab).subscribe())
        })
    }

    fn quit(&self) -> BoxFuture<'_, BackendResult<()>> {
        self.with_state("quit".into(), |_| Ok(()))
    }
}

/// A `Ctx` for `caller` on top of `backend` with a fresh event bus.
pub fn ctx(caller: Caller, backend: Arc<dyn BrowserBackend>) -> Ctx {
    Ctx::new(caller, backend, EventBus::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_cdp_events_reach_subscribers() {
        let fake = FakeBackend::with_tabs(&["https://a.test"]);
        let mut events = fake.cdp_events(TabId(1)).await.unwrap();
        assert_eq!(
            fake.emit_cdp_event(
                TabId(1),
                "Network.requestWillBeSent",
                json!({"requestId": "1"})
            ),
            1
        );
        let ev = events.recv().await.unwrap();
        assert_eq!(ev.method, "Network.requestWillBeSent");
        assert!(fake.cdp_events(TabId(9)).await.is_err());
    }
}
