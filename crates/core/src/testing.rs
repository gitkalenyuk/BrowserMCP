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
use crate::tabs::TabStrip;

/// A browser with one window. Navigation finishes instantly; the page title becomes the URL;
/// `Runtime.evaluate` returns the text set with [`FakeBackend::set_text`]. Tab order and the
/// active tab follow the real rules ([`TabStrip`]).
///
/// Other CDP methods can be scripted with [`FakeBackend::on_cdp`]; every CDP call is logged with
/// its parameters ([`FakeBackend::cdp_calls`]).
#[derive(Default)]
pub struct FakeBackend {
    state: Mutex<FakeState>,
}

/// Scripted CDP answers: `(tab, method, params)` → `Some(result)` to answer, `None` to fall back
/// to the built-in behaviour.
///
/// * Calls in a child session ([`BrowserBackend::cdp_session`]) reach the handler (and the
///   [`FakeBackend::cdp_calls`] log) with the session in `params["sessionId"]`.
/// * An `Ok` result may carry `"__events": [{"method", "params", "sessionId"?}, ...]`: those
///   events are removed from the result and sent to the tab's event subscribers *before*
///   the call returns, like Chromium does (e.g. `Target.attachedToTarget` during
///   `Target.setAutoAttach`).
pub type CdpHandler = Box<dyn Fn(TabId, &str, &Value) -> Option<BackendResult<Value>> + Send>;

#[derive(Default)]
struct FakeState {
    /// Tab data by id (order lives in `strip`).
    tabs: HashMap<TabId, TabInfo>,
    strip: TabStrip,
    next_id: i32,
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

    /// Backend with one tab per URL (ids 1, 2, …); the first one is active.
    pub fn with_tabs(urls: &[&str]) -> Arc<Self> {
        let fake = Self::default();
        {
            let mut state = fake.lock();
            for url in urls {
                state.add_tab(url.to_string(), true);
            }
            if let Some(first) = state.strip.tabs().first().copied() {
                state.strip.activate(first);
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

    /// Tabs in strip order, with `active` filled in.
    pub fn tabs(&self) -> Vec<TabInfo> {
        self.lock().list()
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
    fn add_tab(&mut self, url: String, background: bool) -> TabId {
        self.next_id += 1;
        let id = TabId(self.next_id);
        self.tabs.insert(id, TabInfo::new(id, url));
        self.strip.insert(id, None);
        if !background {
            self.strip.activate(id);
        }
        id
    }

    fn list(&self) -> Vec<TabInfo> {
        let active = self.strip.active();
        self.strip
            .tabs()
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .map(|t| TabInfo {
                active: Some(t.id) == active,
                ..t.clone()
            })
            .collect()
    }

    fn event_sender(&mut self, tab: TabId) -> tokio::sync::broadcast::Sender<CdpEvent> {
        self.cdp_events
            .entry(tab)
            .or_insert_with(|| tokio::sync::broadcast::channel(1024).0)
            .clone()
    }

    /// Send the `"__events"` a scripted answer asked for (see [`CdpHandler`]) and return the
    /// answer without them.
    fn send_scripted_events(&mut self, tab: TabId, mut result: Value) -> Value {
        let events = result
            .as_object_mut()
            .and_then(|o| o.remove("__events"))
            .and_then(|e| e.as_array().cloned())
            .unwrap_or_default();
        if !events.is_empty() {
            let sender = self.event_sender(tab);
            for event in events {
                let _ = sender.send(CdpEvent {
                    method: event["method"].as_str().unwrap_or_default().to_string(),
                    params: event["params"].clone(),
                    session_id: event["sessionId"].as_str().map(str::to_string),
                });
            }
        }
        result
    }

    fn tab_mut(&mut self, tab: TabId) -> BackendResult<&mut TabInfo> {
        self.tabs
            .get_mut(&tab)
            .ok_or(BackendError::TabNotFound(tab))
    }
}

impl BrowserBackend for FakeBackend {
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>> {
        self.with_state("list_tabs".into(), |s| Ok(s.list()))
    }

    fn open_tab(&self, url: String, background: bool) -> BoxFuture<'_, BackendResult<TabId>> {
        let call = if background {
            format!("open_tab {url} background")
        } else {
            format!("open_tab {url}")
        };
        self.with_state(call, |s| Ok(s.add_tab(url, background)))
    }

    fn close_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        self.with_state(format!("close_tab {tab}"), |s| {
            s.tab_mut(tab)?;
            s.tabs.remove(&tab);
            s.strip.remove(tab);
            Ok(())
        })
    }

    fn activate_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        self.with_state(format!("activate_tab {tab}"), |s| {
            s.tab_mut(tab)?;
            s.strip.activate(tab);
            Ok(())
        })
    }

    fn stop_loading(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        self.with_state(format!("stop_loading {tab}"), |s| {
            s.tab_mut(tab)?.loading = false;
            Ok(())
        })
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
            info.can_go_back = true;
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
                return answer.map(|result| s.send_scripted_events(tab, result));
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

    fn cdp_session(
        &self,
        tab: TabId,
        session_id: Option<String>,
        method: String,
        mut params: Value,
    ) -> BoxFuture<'_, BackendResult<Value>> {
        let Some(session) = session_id else {
            return self.cdp(tab, method, params);
        };
        // Same as `cdp`, with the session visible to the handler and in the log.
        if !params.is_object() {
            params = json!({});
        }
        params["sessionId"] = Value::String(session);
        self.cdp(tab, method, params)
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

    #[tokio::test]
    async fn fake_sessions_and_scripted_events() {
        let fake = FakeBackend::with_tabs(&["https://a.test"]);
        fake.on_cdp(|_tab, method, params| match method {
            "Target.setAutoAttach" => Some(Ok(json!({ "__events": [{
                "method": "Target.attachedToTarget",
                "params": { "sessionId": "S1" },
            }]}))),
            "DOM.getDocument" => Some(Ok(json!({ "session": params["sessionId"] }))),
            _ => None,
        });
        let mut events = fake.cdp_events(TabId(1)).await.unwrap();
        let reply = fake
            .cdp(TabId(1), "Target.setAutoAttach".into(), json!({}))
            .await
            .unwrap();
        // The event arrived before the reply, and the reply no longer carries it.
        assert_eq!(reply, json!({}));
        assert_eq!(events.try_recv().unwrap().params["sessionId"], "S1");
        let reply = fake
            .cdp_session(
                TabId(1),
                Some("S1".into()),
                "DOM.getDocument".into(),
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(reply["session"], "S1");
        assert_eq!(fake.cdp_calls()[1].1["sessionId"], "S1");
    }
}
