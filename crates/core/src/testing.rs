//! Test helpers: an in-memory [`FakeBackend`].
//!
//! Compiled for this crate's tests, and for other crates with the `testing` feature:
//! `bmcp-core = { workspace = true, features = ["testing"] }` under `[dev-dependencies]`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::backend::{BackendError, BackendResult, BoxFuture, BrowserBackend, TabId, TabInfo};
use crate::command::{Caller, Ctx};
use crate::events::EventBus;

/// A browser that keeps tabs in a `Vec`. Navigation finishes instantly; the page title becomes
/// the URL; `Runtime.evaluate` returns the text set with [`FakeBackend::set_text`].
#[derive(Default)]
pub struct FakeBackend {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    tabs: Vec<TabInfo>,
    texts: HashMap<TabId, String>,
    /// Returned (once) by the next backend call instead of doing the work.
    fail_next: Option<BackendError>,
    /// Log like `"navigate 1 https://a.test"`, to assert what the backend was asked to do.
    calls: Vec<String>,
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
        _params: Value,
    ) -> BoxFuture<'_, BackendResult<Value>> {
        self.with_state(format!("cdp {tab} {method}"), |s| {
            s.tab_mut(tab)?;
            match method.as_str() {
                "Runtime.evaluate" => {
                    let text = s.texts.get(&tab).cloned().unwrap_or_default();
                    Ok(json!({ "result": { "type": "string", "value": text } }))
                }
                _ => Err(BackendError::Cdp(format!("{method} is not faked"))),
            }
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
