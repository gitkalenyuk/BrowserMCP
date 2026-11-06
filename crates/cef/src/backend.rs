//! [`BrowserBackend`] implemented on top of CEF via the UI-thread bridge.

use std::sync::Arc;
use std::time::Duration;

use bmcp_cdp::CdpClient;
use bmcp_core::{
    BackendError, BackendResult, BoxFuture, BrowserBackend, CdpEvents, TabId, TabInfo,
};
use cef::*;
use serde_json::Value;
use tokio::sync::watch;

use crate::bridge::{LoadState, run_on_ui, tab_created, with_ui};

#[derive(Clone, Default)]
pub struct CefBackend;

impl CefBackend {
    async fn tab_cdp(&self, tab: TabId) -> BackendResult<Arc<CdpClient>> {
        run_on_ui(move || with_ui(|ui| ui.tabs.get(&tab).map(|t| t.cdp.clone())))
            .await?
            .ok_or(BackendError::TabNotFound(tab))
    }

    async fn tab_info(&self, tab: TabId) -> BackendResult<TabInfo> {
        run_on_ui(move || with_ui(|ui| ui.tabs.get(&tab).map(|t| t.info(tab))))
            .await?
            .ok_or(BackendError::TabNotFound(tab))
    }
}

impl BrowserBackend for CefBackend {
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>> {
        Box::pin(run_on_ui(|| {
            with_ui(|ui| ui.tabs.iter().map(|(id, t)| t.info(*id)).collect())
        }))
    }

    fn open_tab(&self, url: String) -> BoxFuture<'_, BackendResult<TabId>> {
        Box::pin(async move {
            // Subscribe before creating so the creation event cannot be missed.
            let mut created = tab_created().subscribe();
            run_on_ui(move || {
                let Some(shell) = with_ui(|ui| ui.shell.clone()) else {
                    return Err(BackendError::ShuttingDown);
                };
                crate::shell::open_window(&url, &shell);
                Ok(())
            })
            .await??;
            let timeout = Duration::from_secs(10);
            match tokio::time::timeout(timeout, created.recv()).await {
                Ok(Ok(id)) => Ok(id),
                Ok(Err(_)) => Err(BackendError::ShuttingDown),
                Err(_) => Err(BackendError::Timeout(timeout)),
            }
        })
    }

    fn navigate(
        &self,
        tab: TabId,
        url: String,
        timeout: Duration,
    ) -> BoxFuture<'_, BackendResult<TabInfo>> {
        Box::pin(async move {
            let started = run_on_ui(move || {
                let entry = with_ui(|ui| {
                    ui.tabs
                        .get(&tab)
                        .map(|t| (t.browser.clone(), t.load.subscribe()))
                });
                let (browser, load) = entry.ok_or(BackendError::TabNotFound(tab))?;
                let start_seq = load.borrow().seq;
                let frame = browser.main_frame().ok_or(BackendError::TabNotFound(tab))?;
                frame.load_url(Some(&CefString::from(url.as_str())));
                Ok::<_, BackendError>((load, start_seq))
            })
            .await??;
            wait_for_load(started.0, started.1, timeout).await?;
            self.tab_info(tab).await
        })
    }

    fn cdp(
        &self,
        tab: TabId,
        method: String,
        params: Value,
    ) -> BoxFuture<'_, BackendResult<Value>> {
        Box::pin(async move {
            let client = self.tab_cdp(tab).await?;
            client
                .call(&method, params)
                .await
                .map_err(|e| BackendError::Cdp(e.to_string()))
        })
    }

    fn cdp_events(&self, tab: TabId) -> BoxFuture<'_, BackendResult<CdpEvents>> {
        Box::pin(async move { Ok(self.tab_cdp(tab).await?.subscribe()) })
    }

    fn quit(&self) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(run_on_ui(|| {
            let browsers: Vec<Browser> =
                with_ui(|ui| ui.tabs.values().map(|t| t.browser.clone()).collect());
            for mut browser in browsers {
                // Views-hosted browsers must be closed through their window; closing only
                // the browser leaves an empty window and cef::shutdown() then never returns.
                let window =
                    browser_view_get_for_browser(Some(&mut browser)).and_then(|v| v.window());
                match window {
                    Some(window) => window.close(),
                    None => {
                        if let Some(host) = browser.host() {
                            host.close_browser(1);
                        }
                    }
                }
            }
        }))
    }
}

/// Wait until a load that started after `start_seq` has finished.
async fn wait_for_load(
    mut load: watch::Receiver<LoadState>,
    start_seq: u64,
    timeout: Duration,
) -> BackendResult<()> {
    let done = load.wait_for(|s: &LoadState| s.seq > start_seq && !s.loading);
    match tokio::time::timeout(timeout, done).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(BackendError::ShuttingDown), // tab closed while loading
        Err(_) => Err(BackendError::Timeout(timeout)),
    }
}
