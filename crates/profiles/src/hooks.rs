//! Preparing a new tab before its first navigation: apply the profile's identity, to the page
//! and to its workers.
//!
//! CDP overrides (user agent, timezone, …) belong to one tab's DevTools session, so every new
//! tab of a profile with an identity must get them again — before the page's first script
//! runs. The CEF crate therefore creates such tabs with `about:blank`, awaits
//! [`TabPreparer::prepare`], and only then navigates to the real URL. Popups of such tabs are
//! re-opened the same way.
//!
//! Workers are separate CDP targets that `Page.addScriptToEvaluateOnNewDocument` does not
//! reach. `prepare` holds `Target.setAutoAttach {waitForDebuggerOnStart: true}` on the tab
//! through the shared owner in bmcp-cdpshare (the OOPIF support of bmcp-browser holds it too,
//! without pausing; the session pauses while anybody wants it), so every new worker starts
//! paused. [`watch_workers`] then sends it bmcp-identity's
//! [`worker_commands`](bmcp_identity::worker_commands) — in order, the last one
//! (`Runtime.runIfWaitingForDebugger`) always, also for other paused targets such as iframes.

use std::sync::Arc;

use bmcp_cdpshare::CdpShare;
use bmcp_core::{BackendResult, BoxFuture, BrowserBackend, Caller, Ctx, EventBus, Registry, TabId};
use bmcp_identity::{Identity, Services, worker_commands};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

/// Our name as an auto-attach holder (see [`bmcp_cdpshare::attach`]).
pub const ATTACH_HOLDER: &str = "identity";

/// The CEF crate's way to ask "does this profile want its tabs prepared?" and to do it.
pub trait TabPreparer: Send + Sync + 'static {
    /// True when new tabs of `profile` must be prepared before their first navigation.
    /// Called on the browser's UI thread: must be quick and never block on async work.
    fn wants(&self, profile: &str) -> bool;

    /// Prepare `tab` (still on `about:blank`): identity applied, workers watched. Resolves
    /// once the tab may navigate; true when the identity was applied.
    fn prepare(&self, tab: TabId, profile: String) -> BoxFuture<'static, bool>;
}

/// The real preparer: the identity assigned to the profile (`identity.assign`,
/// `profile.create {identity}`, a mass-run row's fitted identity) is applied with
/// `stealth.fingerprint_set`, then the tab's workers are watched.
pub struct IdentityPreparer {
    registry: Arc<Registry>,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
}

impl IdentityPreparer {
    pub fn new(
        registry: Arc<Registry>,
        backend: Arc<dyn BrowserBackend>,
        events: EventBus,
    ) -> Self {
        IdentityPreparer {
            registry,
            backend,
            events,
        }
    }

    fn services(&self) -> Option<Arc<Services>> {
        self.registry.extensions().get::<Services>()
    }
}

impl TabPreparer for IdentityPreparer {
    fn wants(&self, profile: &str) -> bool {
        self.services()
            .and_then(|s| s.identity_for_profile(profile).ok().flatten())
            .is_some()
    }

    fn prepare(&self, tab: TabId, profile: String) -> BoxFuture<'static, bool> {
        let services = self.services();
        let registry = self.registry.clone();
        let backend = self.backend.clone();
        let ctx = Ctx::new(Caller::Ui, self.backend.clone(), self.events.clone());
        let share = registry.extensions().get_or_insert_with(CdpShare::default);
        Box::pin(async move {
            let Some(services) = services else {
                return false;
            };
            let identity = match services.identity_for_profile(&profile) {
                Ok(Some(identity)) => identity,
                Ok(None) => return false,
                Err(err) => {
                    tracing::warn!(%tab, profile, %err, "identity of the profile not found");
                    return false;
                }
            };
            let args = json!({ "tab": tab, "identity_id": identity.id });
            match registry.call(ctx, "stealth.fingerprint_set", args).await {
                Ok(_) => tracing::debug!(%tab, profile, identity = identity.id, "identity applied"),
                Err(err) => {
                    tracing::warn!(%tab, profile, identity = identity.id, %err, "identity not applied");
                    return false;
                }
            }
            // What was applied (already adapted to the engine's version); the stored one if
            // the tab lost it meanwhile.
            let applied = services.tabs.get(tab).map_or(identity, |a| a.identity);
            if let Err(err) = watch_workers(backend, share, services, tab, applied).await {
                tracing::warn!(%tab, %err, "worker auto-attach not available");
            }
            true
        })
    }
}

/// Pause the tab's new child targets (shared auto-attach) and give each one `identity`
/// (workers) or just let it run (anything else). Returns once auto-attach is on; the watch
/// ends when the tab closes. Each attach uses the identity applied to the tab at that time
/// (`fallback` when there is none).
pub async fn watch_workers(
    backend: Arc<dyn BrowserBackend>,
    share: Arc<CdpShare>,
    services: Arc<Services>,
    tab: TabId,
    fallback: Identity,
) -> BackendResult<()> {
    // Subscribe first, so no attach event is missed.
    let mut events = backend.cdp_events(tab).await?;
    let switched_on = share
        .hold_auto_attach(&backend, tab, None, ATTACH_HOLDER, true)
        .await?;
    if !switched_on {
        // On already (another holder): get the existing targets announced to us too.
        share.reannounce_auto_attach(&backend, tab, None).await?;
    }
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.method == "Target.attachedToTarget" => {
                    let identity = services
                        .tabs
                        .get(tab)
                        .map_or_else(|| fallback.clone(), |a| a.identity);
                    let backend = backend.clone();
                    tokio::spawn(async move {
                        patch_attached(&*backend, tab, &event.params, &identity).await;
                    });
                }
                Ok(_) => {}
                Err(RecvError::Lagged(lost)) => {
                    // A lost attach of a paused target would leave it hanging.
                    tracing::warn!(%tab, lost, "worker watcher lost CDP events; re-announcing");
                    let _ = share.reannounce_auto_attach(&backend, tab, None).await;
                }
                Err(RecvError::Closed) => break, // tab closed
            }
        }
    });
    Ok(())
}

/// One `Target.attachedToTarget {sessionId, targetInfo: {type}}`: send bmcp-identity's
/// [`worker_commands`] in that session, in order. A failure is never fatal (a warning unless
/// the command is optional), and the last command — `Runtime.runIfWaitingForDebugger`, the
/// only one for targets that are not workers — is always sent. Returns the methods sent.
pub async fn patch_attached(
    backend: &dyn BrowserBackend,
    tab: TabId,
    params: &Value,
    identity: &Identity,
) -> Vec<&'static str> {
    let Some(session) = params["sessionId"].as_str() else {
        return Vec::new();
    };
    let kind = params["targetInfo"]["type"].as_str().unwrap_or_default();
    let mut sent = Vec::new();
    for cmd in worker_commands(identity, kind, None) {
        sent.push(cmd.method);
        let result = backend
            .cdp_session(
                tab,
                Some(session.to_string()),
                cmd.method.to_string(),
                cmd.params,
            )
            .await;
        if let Err(err) = result
            && !cmd.optional
        {
            tracing::warn!(%tab, kind, method = cmd.method, %err, "worker command failed");
        }
    }
    tracing::debug!(%tab, kind, commands = sent.len(), "attached target handled");
    sent
}
