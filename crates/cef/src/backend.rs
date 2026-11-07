//! [`BrowserBackend`] implemented on top of CEF via the UI-thread bridge.

use std::sync::Arc;
use std::time::Duration;

use bmcp_cdp::CdpClient;
use bmcp_core::{
    BackendError, BackendResult, BoxFuture, BrowserBackend, CdpEvents, OpenTabOptions, TabId,
    TabInfo, WindowTarget, ZoomChange,
};
use bmcp_profiles::{
    CookieInfo, CookieTarget, OpenProfile, PermissionPrompt, ProfileBackend, ProfileSettings,
};
use cef::*;
use serde_json::Value;
use tokio::sync::{oneshot, watch};

use crate::bridge::{LoadState, run_on_ui, with_ui};

#[derive(Clone, Default)]
pub struct CefBackend;

impl CefBackend {
    async fn tab_cdp(&self, tab: TabId) -> BackendResult<Arc<CdpClient>> {
        run_on_ui(move || with_ui(|ui| ui.tabs.get(&tab).map(|t| t.cdp.clone())))
            .await?
            .ok_or(BackendError::TabNotFound(tab))
    }

    async fn tab_info(&self, tab: TabId) -> BackendResult<TabInfo> {
        run_on_ui(move || crate::bridge::tab_info(tab))
            .await?
            .ok_or(BackendError::TabNotFound(tab))
    }

    /// Run `action` on the tab's browser (UI thread).
    async fn with_browser(
        &self,
        tab: TabId,
        action: impl FnOnce(&Browser) + Send + 'static,
    ) -> BackendResult<()> {
        run_on_ui(move || {
            let browser = with_ui(|ui| ui.tabs.get(&tab).map(|t| t.browser.clone()))
                .ok_or(BackendError::TabNotFound(tab))?;
            action(&browser);
            Ok(())
        })
        .await?
    }
}

impl BrowserBackend for CefBackend {
    fn list_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<TabInfo>>> {
        Box::pin(run_on_ui(crate::bridge::list_tabs))
    }

    fn open_tab(&self, url: String, background: bool) -> BoxFuture<'_, BackendResult<TabId>> {
        self.open_tab_with(
            url,
            OpenTabOptions {
                background,
                ..Default::default()
            },
        )
    }

    fn open_tab_with(
        &self,
        url: String,
        mut options: OpenTabOptions,
    ) -> BoxFuture<'_, BackendResult<TabId>> {
        Box::pin(async move {
            // Headless: every tab is off-screen, and the new tab page (a chrome page) is blank.
            let url = if crate::osr::headless() {
                options.window = WindowTarget::Offscreen;
                if url.starts_with("bmcp:") {
                    "about:blank".to_owned()
                } else {
                    url
                }
            } else {
                url
            };
            // Our own pages (the new tab page) make no sense without a window.
            if options.window == WindowTarget::Offscreen && url.starts_with("bmcp:") {
                return Err(BackendError::Invalid(
                    "off-screen tabs show web pages only; pass a url".into(),
                ));
            }
            // `register_tab` answers on this channel once the browser exists (and, for a
            // profile with an identity, once the identity is applied and the page loads).
            let (reply, created) = oneshot::channel();
            run_on_ui(move || crate::shell::open_tab(url, options, reply)).await??;
            let timeout = Duration::from_secs(30);
            match tokio::time::timeout(timeout, created).await {
                Ok(Ok(id)) => Ok(id),
                Ok(Err(_)) => Err(BackendError::ShuttingDown),
                Err(_) => Err(BackendError::Timeout(timeout)),
            }
        })
    }

    fn close_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        // Settings → Startup & tabs: "Keep the window when its last tab closes".
        if crate::settings().is_some_and(|s| s.startup().keep_window_on_last_tab) {
            return self.close_tab_keep_window(tab);
        }
        Box::pin(async move { run_on_ui(move || crate::chrome::close_tab(tab)).await? })
    }

    fn close_tab_keep_window(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        use crate::chrome::KeepWindowPlan;
        Box::pin(async move {
            let plan = run_on_ui(move || crate::chrome::plan_keep_window(tab)).await??;
            match plan {
                KeepWindowPlan::Close => {}
                KeepWindowPlan::ShowNewTabInstead => {
                    tracing::info!(%tab, "last tab of a window: showing the new tab page instead");
                    return run_on_ui(move || crate::chrome::show_new_tab(tab)).await?;
                }
                KeepWindowPlan::ReplaceThenClose(key) => {
                    tracing::info!(%tab, window = key, "last tab of a window: new tab page first");
                    // Wait until the new tab is registered: otherwise the closing tab would
                    // still be the window's only tab and take the window with it.
                    let (reply, created) = oneshot::channel();
                    run_on_ui(move || crate::chrome::open_new_tab_in(key, reply)).await??;
                    let timeout = Duration::from_secs(10);
                    match tokio::time::timeout(timeout, created).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(_)) => return Err(BackendError::ShuttingDown),
                        Err(_) => return Err(BackendError::Timeout(timeout)),
                    }
                }
            }
            run_on_ui(move || crate::chrome::close_tab(tab)).await?
        })
    }

    fn activate_tab(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(async move { run_on_ui(move || crate::chrome::activate_tab(tab)).await? })
    }

    fn stop_loading(&self, tab: TabId) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(self.with_browser(tab, |browser| browser.stop_load()))
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
            // Off-screen tabs: a dialog held by `osr::dialog_handler` is answered through its
            // CEF callback (the same path as a click in a window's dialog).
            if method == "Page.handleJavaScriptDialog" {
                let accept = params["accept"].as_bool().unwrap_or(false);
                let text = params["promptText"].as_str().map(str::to_owned);
                let answered =
                    run_on_ui(move || crate::osr::answer_dialog(tab.0, accept, text.as_deref()))
                        .await?;
                if answered {
                    return Ok(Value::Object(Default::default()));
                }
            }
            let client = self.tab_cdp(tab).await?;
            client
                .call(&method, params)
                .await
                .map_err(|e| BackendError::Cdp(e.to_string()))
        })
    }

    fn cdp_session(
        &self,
        tab: TabId,
        session_id: Option<String>,
        method: String,
        params: Value,
    ) -> BoxFuture<'_, BackendResult<Value>> {
        Box::pin(async move {
            let client = self.tab_cdp(tab).await?;
            client
                .call_in_session(session_id.as_deref(), &method, params)
                .await
                .map_err(|e| BackendError::Cdp(e.to_string()))
        })
    }

    fn cdp_events(&self, tab: TabId) -> BoxFuture<'_, BackendResult<CdpEvents>> {
        Box::pin(async move { Ok(self.tab_cdp(tab).await?.subscribe()) })
    }

    fn split_tabs(
        &self,
        left: TabId,
        right: TabId,
        ratio: Option<f64>,
    ) -> BoxFuture<'_, BackendResult<()>> {
        let ratio = bmcp_core::commands::tabs::clamp_split_ratio(ratio);
        Box::pin(
            async move { run_on_ui(move || crate::chrome::split_tabs(left, right, ratio)).await? },
        )
    }

    fn unsplit(&self, tab: Option<TabId>) -> BoxFuture<'_, BackendResult<Vec<TabId>>> {
        Box::pin(async move { run_on_ui(move || crate::chrome::unsplit(tab)).await? })
    }

    fn restore_closed(&self) -> BoxFuture<'_, BackendResult<Option<TabId>>> {
        self.restore_closed_at(0)
    }

    fn closed_tabs(&self) -> BoxFuture<'_, BackendResult<Vec<bmcp_core::ClosedTabInfo>>> {
        Box::pin(run_on_ui(crate::session::closed_list))
    }

    fn clear_closed(&self) -> BoxFuture<'_, BackendResult<usize>> {
        Box::pin(run_on_ui(crate::session::clear_closed))
    }

    fn zoom_levels(&self) -> BoxFuture<'_, BackendResult<Vec<(String, f64)>>> {
        Box::pin(run_on_ui(crate::zoom::levels))
    }

    fn reset_zoom(&self, host: Option<String>) -> BoxFuture<'_, BackendResult<Vec<String>>> {
        Box::pin(run_on_ui(move || crate::zoom::reset(host.as_deref())))
    }

    fn restore_closed_at(&self, index: usize) -> BoxFuture<'_, BackendResult<Option<TabId>>> {
        Box::pin(async move {
            let (reply, created) = oneshot::channel();
            let started = run_on_ui(move || crate::session::reopen_closed(index, reply)).await??;
            if !started {
                return Ok(None);
            }
            let timeout = Duration::from_secs(30);
            match tokio::time::timeout(timeout, created).await {
                Ok(Ok(id)) => Ok(Some(id)),
                Ok(Err(_)) => Err(BackendError::ShuttingDown),
                Err(_) => Err(BackendError::Timeout(timeout)),
            }
        })
    }

    fn zoom(&self, tab: TabId, change: ZoomChange) -> BoxFuture<'_, BackendResult<f64>> {
        Box::pin(async move { run_on_ui(move || crate::zoom::change(tab, change)).await? })
    }

    fn quit(&self) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(run_on_ui(|| {
            tracing::info!("quit requested: closing every window");
            if crate::osr::headless() {
                // Set before closing: the last tab closing then ends the loop (or now, if none).
                let open = with_ui(|ui| ui.shell.as_ref().map_or(0, |s| s.browsers.open_count()));
                crate::osr::begin_quit(open);
            }
            // Views-hosted browsers must be closed through their window; closing only
            // the browser leaves an empty window and cef::shutdown() then never returns.
            let mut windows: Vec<Window> = with_ui(|ui| {
                ui.windows
                    .values()
                    .map(|w| w.views.window.clone())
                    .collect()
            });
            let single: Vec<Browser> = with_ui(|ui| {
                ui.tabs
                    .values()
                    .filter(|t| t.window.is_none())
                    .map(|t| t.browser.clone())
                    .collect()
            });
            for mut browser in single {
                if let Some(window) =
                    browser_view_get_for_browser(Some(&mut browser)).and_then(|v| v.window())
                {
                    windows.push(window);
                }
            }
            // Off-screen tabs have no window: close their browsers directly.
            let offscreen: Vec<Browser> = with_ui(|ui| {
                ui.tabs
                    .values()
                    .filter(|t| t.offscreen)
                    .map(|t| t.browser.clone())
                    .collect()
            });
            for browser in offscreen {
                if let Some(host) = browser.host() {
                    host.close_browser(1);
                }
            }
            for window in windows {
                window.close();
            }
            crate::devtools::close_all();
        }))
    }
}

/// Profiles, cookies (see `profiles.rs`, `cookies.rs`), for the `profile.*` / `cookies.*`
/// commands of bmcp-profiles.
impl ProfileBackend for CefBackend {
    fn configure(&self, settings: ProfileSettings) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(run_on_ui(move || crate::profiles::configure(settings)))
    }

    fn forget(&self, id: String) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(async move { run_on_ui(move || crate::profiles::forget(&id)).await? })
    }

    fn open_profiles(&self) -> BoxFuture<'_, BackendResult<Vec<OpenProfile>>> {
        Box::pin(run_on_ui(|| {
            with_ui(|ui| {
                // Every profile with a context or a tab (the launch profile always).
                let mut out: Vec<OpenProfile> = Vec::new();
                let mut add = |key: &crate::profiles::ProfileKey, tab: Option<TabId>| {
                    let (id, incognito) = (key.name(), key.incognito);
                    match out
                        .iter_mut()
                        .find(|p| p.id == id && p.incognito == incognito)
                    {
                        Some(p) => p.tabs.extend(tab),
                        None => out.push(OpenProfile {
                            id,
                            incognito,
                            tabs: tab.into_iter().collect(),
                        }),
                    }
                };
                add(&crate::profiles::ProfileKey::default(), None);
                for key in ui.profiles.contexts.keys() {
                    add(key, None);
                }
                for (id, tab) in &ui.tabs {
                    add(&tab.profile, Some(*id));
                }
                out
            })
        }))
    }

    fn cookies(
        &self,
        target: CookieTarget,
        domain: Option<String>,
    ) -> BoxFuture<'_, BackendResult<Vec<CookieInfo>>> {
        Box::pin(crate::cookies::get(target, domain))
    }

    fn set_cookies(
        &self,
        target: CookieTarget,
        cookies: Vec<CookieInfo>,
    ) -> BoxFuture<'_, BackendResult<Vec<bool>>> {
        Box::pin(crate::cookies::set(target, cookies))
    }

    fn delete_cookies(
        &self,
        target: CookieTarget,
        domain: Option<String>,
        name: Option<String>,
    ) -> BoxFuture<'_, BackendResult<usize>> {
        Box::pin(crate::cookies::delete(target, domain, name))
    }

    fn pending_permissions(&self) -> BoxFuture<'_, BackendResult<Vec<PermissionPrompt>>> {
        Box::pin(run_on_ui(crate::prompts::pending))
    }

    fn answer_permission(
        &self,
        id: u64,
        allow: bool,
    ) -> BoxFuture<'_, BackendResult<Option<PermissionPrompt>>> {
        Box::pin(run_on_ui(move || crate::prompts::answer(id, allow)))
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
