//! Preparing a new tab before its first navigation: apply the profile's identity.
//!
//! CDP overrides (user agent, timezone, …) belong to one tab's DevTools session, so every new
//! tab of a profile with an identity must get them again — before the page's first script
//! runs. The CEF crate therefore creates such tabs with `about:blank`, calls
//! [`TabPreparer::prepare`], and only then navigates to the real URL. Popups of such tabs are
//! re-opened the same way.
//!
//! Workers are separate CDP targets that `Page.addScriptToEvaluateOnNewDocument` does not
//! reach. `prepare` also returns [`WorkerOverrides`]; the CEF crate auto-attaches to the
//! tab's workers (`Target.setAutoAttach {waitForDebuggerOnStart, flatten}`) and applies them
//! before the worker's own script runs.

use std::sync::Arc;

use bmcp_core::{BoxFuture, BrowserBackend, Caller, Ctx, EventBus, Registry, TabId};
use bmcp_identity::{Identity, Services};
use serde_json::{Value, json};

use crate::prefs::strip_q_values;

/// What a worker of a prepared tab gets.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerOverrides {
    /// `Network.setUserAgentOverride` parameters (UA, platform, Accept-Language, UA-CH).
    pub user_agent: Value,
    /// Evaluated in the worker before its script runs (`navigator.*` getters).
    pub script: String,
}

impl WorkerOverrides {
    pub fn for_identity(identity: &Identity) -> Self {
        WorkerOverrides {
            user_agent: bmcp_identity::stealth::user_agent_params(identity),
            script: worker_patch_js(
                identity.hardware_concurrency,
                identity.device_memory,
                &identity.accept_language,
            ),
        }
    }
}

/// Getters on `WorkerNavigator.prototype` for cores, memory and languages (what a
/// fingerprinting script compares between page and worker).
pub fn worker_patch_js(cores: u32, memory: f64, accept_language: &str) -> String {
    let languages: Vec<String> = strip_q_values(accept_language)
        .split(',')
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let first = languages.first().cloned().unwrap_or_else(|| "en-US".into());
    format!(
        "(() => {{ if (typeof WorkerNavigator === 'undefined') return; \
         const P = WorkerNavigator.prototype; \
         const def = (k, f) => {{ try {{ Object.defineProperty(P, k, {{ get: f, configurable: true, enumerable: true }}); }} catch (e) {{}} }}; \
         const langs = Object.freeze({langs}); \
         def('hardwareConcurrency', () => {cores}); \
         def('deviceMemory', () => {memory}); \
         def('languages', () => langs); \
         def('language', () => {first}); }})();",
        langs = json!(languages),
        first = json!(first),
    )
}

/// The CEF crate's way to ask "does this profile want its tabs prepared?" and to do it.
pub trait TabPreparer: Send + Sync + 'static {
    /// True when new tabs of `profile` must be prepared before their first navigation.
    /// Called on the browser's UI thread: must be quick and never block on async work.
    fn wants(&self, profile: &str) -> bool;

    /// Prepare `tab` (still on `about:blank`). Returns what its workers should get.
    fn prepare(&self, tab: TabId, profile: String) -> BoxFuture<'static, Option<WorkerOverrides>>;
}

/// The real preparer: the identity assigned to the profile (`identity.assign`,
/// `profile.create {identity}`) is applied with `stealth.fingerprint_set`.
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

    /// Identity id assigned to `profile`, if any.
    fn assigned(&self, profile: &str) -> Option<String> {
        let services = self.services()?;
        let file = services.identities.read().ok()?;
        file.profiles.get(profile).cloned()
    }
}

impl TabPreparer for IdentityPreparer {
    fn wants(&self, profile: &str) -> bool {
        self.assigned(profile).is_some()
    }

    fn prepare(&self, tab: TabId, profile: String) -> BoxFuture<'static, Option<WorkerOverrides>> {
        let identity_id = self.assigned(&profile);
        let services = self.services();
        let registry = self.registry.clone();
        let ctx = Ctx::new(Caller::Ui, self.backend.clone(), self.events.clone());
        Box::pin(async move {
            let identity_id = identity_id?;
            let args = json!({ "tab": tab, "identity_id": identity_id });
            match registry.call(ctx, "stealth.fingerprint_set", args).await {
                Ok(_) => tracing::debug!(%tab, profile, identity_id, "identity applied"),
                Err(err) => {
                    tracing::warn!(%tab, profile, identity_id, %err, "identity not applied");
                    return None;
                }
            }
            // What was applied (already adapted to the engine's version).
            let applied = services?.tabs.get(tab)?;
            Some(WorkerOverrides::for_identity(&applied.identity))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_script_has_the_values() {
        let js = worker_patch_js(6, 8.0, "de-DE,de;q=0.9,en;q=0.8");
        assert!(js.contains("'hardwareConcurrency', () => 6"));
        assert!(js.contains("'deviceMemory', () => 8"));
        assert!(js.contains(r#"Object.freeze(["de-DE","de","en"])"#));
        assert!(js.contains(r#"'language', () => "de-DE""#));
    }
}
