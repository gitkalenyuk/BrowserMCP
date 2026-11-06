//! Preparing new tabs before their first navigation (identity auto-apply, incl. workers).
//!
//! Flow for a tab of a profile whose [`TabPreparer::wants`] it:
//! 1. the browser is created with `about:blank` (`PendingTab::then_load` = the real URL);
//! 2. `bridge::register_tab` calls [`prepare_then_load`] on the tokio runtime;
//! 3. the preparer (`bmcp_profiles::IdentityPreparer`) applies the identity
//!    (`stealth.fingerprint_set`) and starts patching the tab's workers (shared
//!    `Target.setAutoAttach {waitForDebuggerOnStart}` + bmcp-identity's worker commands);
//! 4. only then the real URL is loaded, and `tabs.open` gets its answer.

use std::sync::{Arc, RwLock};

use bmcp_core::TabId;
use bmcp_profiles::TabPreparer;
use cef::{ImplBrowser, ImplFrame};
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
    hooks.runtime.spawn(async move {
        preparer.prepare(tab, profile).await;
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
