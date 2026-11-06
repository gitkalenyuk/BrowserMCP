//! Preparing new tabs before their first navigation (identity auto-apply) and applying
//! identity overrides to the tab's workers.
//!
//! Flow for a tab of a profile whose [`TabPreparer::wants`] it:
//! 1. the browser is created with `about:blank` (`PendingTab::then_load` = the real URL);
//! 2. `bridge::register_tab` calls [`prepare_then_load`] on the tokio runtime;
//! 3. the preparer applies the identity (`stealth.fingerprint_set`) and returns the worker
//!    overrides; we enable `Target.setAutoAttach {waitForDebuggerOnStart, flatten}` and
//!    patch each worker before it runs ([`watch_workers`]);
//! 4. only then the real URL is loaded, and `tabs.open` gets its answer.

use std::sync::{Arc, RwLock};

use bmcp_cdp::CdpClient;
use bmcp_core::TabId;
use bmcp_profiles::{TabPreparer, WorkerOverrides};
use cef::{ImplBrowser, ImplFrame};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;

use crate::bridge::{run_on_ui, with_ui};

struct Hooks {
    preparer: Arc<dyn TabPreparer>,
    runtime: tokio::runtime::Handle,
}

static HOOKS: RwLock<Option<Hooks>> = RwLock::new(None);

/// Install the preparer (the app does this once the command registry exists). Tabs created
/// before that are not prepared.
pub fn set_tab_preparer(preparer: Arc<dyn TabPreparer>, runtime: tokio::runtime::Handle) {
    *HOOKS.write().unwrap_or_else(|e| e.into_inner()) = Some(Hooks { preparer, runtime });
}

/// Must new tabs of `profile` be prepared before they load anything? UI thread.
pub(crate) fn wants_prepare(profile: &str) -> bool {
    let hooks = HOOKS.read().unwrap_or_else(|e| e.into_inner());
    hooks.as_ref().is_some_and(|h| h.preparer.wants(profile))
}

/// Run step 3 + 4 of the module docs for a freshly registered tab. UI thread.
pub(crate) fn prepare_then_load(
    tab: TabId,
    profile: String,
    url: String,
    reply: Option<oneshot::Sender<TabId>>,
) {
    let hooks = HOOKS.read().unwrap_or_else(|e| e.into_inner());
    let Some(hooks) = hooks.as_ref() else {
        // Cannot happen (then_load is only set when a preparer exists); load anyway.
        load_url(tab, &url);
        if let Some(reply) = reply {
            let _ = reply.send(tab);
        }
        return;
    };
    let preparer = hooks.preparer.clone();
    let cdp = with_ui(|ui| ui.tabs.get(&tab).map(|t| t.cdp.clone()));
    hooks.runtime.spawn(async move {
        let overrides = preparer.prepare(tab, profile).await;
        if let (Some(overrides), Some(cdp)) = (overrides, cdp) {
            watch_workers(cdp, overrides).await;
        }
        let _ = run_on_ui(move || load_url(tab, &url)).await;
        if let Some(reply) = reply {
            let _ = reply.send(tab);
        }
    });
}

/// Navigate a tab's main frame. UI thread.
fn load_url(tab: TabId, url: &str) {
    let browser = with_ui(|ui| ui.tabs.get(&tab).map(|t| t.browser.clone()));
    if let Some(frame) = browser.and_then(|b| b.main_frame()) {
        frame.load_url(Some(&cef::CefString::from(url)));
    }
}

/// Worker target types that get the overrides (the page and iframes are covered by the
/// tab's own session).
pub(crate) fn is_worker_type(kind: &str) -> bool {
    matches!(kind, "worker" | "shared_worker" | "service_worker")
}

/// Auto-attach to the tab's child targets, paused at start. Workers get the user agent and
/// the `navigator` patch, then everything is resumed. Returns once auto-attach is on.
pub(crate) async fn watch_workers(cdp: Arc<CdpClient>, overrides: WorkerOverrides) {
    // Subscribe first, so no attach event is missed.
    let mut events = cdp.subscribe();
    let params = json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true });
    if let Err(err) = cdp.call("Target.setAutoAttach", params).await {
        tracing::warn!(%err, "worker auto-attach not available");
        return;
    }
    let overrides = Arc::new(overrides);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.method == "Target.attachedToTarget" => {
                    let cdp = cdp.clone();
                    let overrides = overrides.clone();
                    tokio::spawn(async move { on_attached(&cdp, &overrides, &event.params).await });
                }
                Ok(_) => {}
                Err(RecvError::Lagged(lost)) => {
                    tracing::warn!(lost, "worker watcher lost CDP events");
                }
                Err(RecvError::Closed) => break, // tab closed
            }
        }
    });
}

/// One new child target: patch it if it is a worker, then let it run.
async fn on_attached(cdp: &CdpClient, overrides: &WorkerOverrides, params: &Value) {
    let Some(session) = params["sessionId"].as_str() else {
        return;
    };
    let kind = params["targetInfo"]["type"].as_str().unwrap_or_default();
    if is_worker_type(kind) {
        let ua = cdp
            .call_in_session(
                Some(session),
                "Network.setUserAgentOverride",
                overrides.user_agent.clone(),
            )
            .await;
        let patch = cdp
            .call_in_session(
                Some(session),
                "Runtime.evaluate",
                json!({ "expression": overrides.script, "silent": true }),
            )
            .await;
        tracing::debug!(
            kind,
            ua_ok = ua.is_ok(),
            patch_ok = patch.is_ok(),
            "worker patched"
        );
    }
    if params["waitingForDebugger"].as_bool() == Some(true) {
        let _ = cdp
            .call_in_session(Some(session), "Runtime.runIfWaitingForDebugger", json!({}))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::is_worker_type;

    #[test]
    fn worker_types() {
        assert!(is_worker_type("worker"));
        assert!(is_worker_type("service_worker"));
        assert!(!is_worker_type("iframe"));
        assert!(!is_worker_type("page"));
    }
}
