//! [`ProfilesService`]: stored profile settings (`data/config/profiles.json`), the link to the
//! browser ([`ProfileBackend`]) and applying a profile's preferences.
//!
//! The proxy of a profile is *not* kept here: it is the profile's routing in bmcp-identity
//! (`proxies.json` routes, the same as `proxy.rules_set`), so `profile.create {proxy}` and
//! `proxy.rules_set` are two views of one setting. `profile.create {proxy}` sets the routing's
//! default; split rules stay. Proxies are always reached through bmcp-identity's local
//! forwarders (passwords never reach Chromium).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use bmcp_core::{CmdError, Ctx, ErrorCode};
use bmcp_identity::Services;
use bmcp_identity::proxy::RuleSet;
use bmcp_identity::store::JsonStore;
use serde::{Deserialize, Serialize};

use crate::backend::{ProfileBackend, ProfileSettings};
use crate::prefs::{Prefs, WebrtcPolicy};

/// Settings of one profile as stored on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Only in files from before the proxy moved to bmcp-identity's routing: moved there
    /// (as the routing default) the next time the profile is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Hosts that skip a fixed proxy (Chromium syntax, `;`-separated, e.g. `<local>;*.lan`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_bypass: Option<String>,
    /// Accept-Language list; default: the assigned identity's, else Chromium's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_language: Option<String>,
    /// Name shown in the UI (the workspace name); default: the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Workspace color in the UI: grey, blue, red, yellow, green, pink, purple, cyan, orange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// `tabs.open {profile}` without `incognito` opens an in-memory tab of this profile
    /// (cookies, history and cache are gone when its last tab closes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incognito_default: bool,
    /// WebRTC addresses (default `auto`: limited to the proxy when proxied).
    #[serde(default, skip_serializing_if = "WebrtcPolicy::is_auto")]
    pub webrtc: WebrtcPolicy,
    #[serde(default)]
    pub created_ms: i64,
}

/// Colors a workspace may have (the UI's tab-group palette).
pub const COLORS: [&str; 9] = [
    "grey", "blue", "red", "yellow", "green", "pink", "purple", "cyan", "orange",
];

/// Contents of `profiles.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilesFile {
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

/// Shared state of the profile commands (in the registry's extensions).
pub struct ProfilesService {
    pub store: JsonStore<ProfilesFile>,
    /// `data/profiles`: one cache folder per profile.
    profiles_dir: PathBuf,
    /// The profile the browser started with (`--profile`, normally `default`).
    pub launch_profile: String,
    /// bmcp-identity (proxy routing, forwarders, identities), set by [`crate::install`].
    identities: RwLock<Option<Arc<Services>>>,
    /// What was handed to the browser last, by profile id.
    applied: Mutex<BTreeMap<String, Prefs>>,
    backend: RwLock<Option<Arc<dyn ProfileBackend>>>,
}

impl ProfilesService {
    /// `data_root` = the data folder (`config/profiles.json`, `profiles/<id>/`).
    pub fn new(data_root: &std::path::Path, launch_profile: &str) -> Self {
        ProfilesService {
            store: JsonStore::new(data_root.join("config").join("profiles.json")),
            profiles_dir: data_root.join("profiles"),
            launch_profile: launch_profile.to_string(),
            identities: RwLock::new(None),
            applied: Mutex::new(BTreeMap::new()),
            backend: RwLock::new(None),
        }
    }

    pub fn from_ctx(ctx: &Ctx) -> Result<Arc<ProfilesService>, CmdError> {
        ctx.extensions.get::<ProfilesService>().ok_or_else(|| {
            CmdError::internal("profile service missing: bmcp_profiles::install was not called")
        })
    }

    /// Connect the browser (done once by the app).
    pub fn set_backend(&self, backend: Arc<dyn ProfileBackend>) {
        *self.backend.write().unwrap_or_else(|e| e.into_inner()) = Some(backend);
    }

    pub fn backend(&self) -> Result<Arc<dyn ProfileBackend>, CmdError> {
        self.backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| {
                CmdError::new(ErrorCode::NotImplemented, "profiles need the real browser")
            })
    }

    /// Use these identity services (done by [`crate::install`]).
    pub fn set_identities(&self, services: Arc<Services>) {
        *self.identities.write().unwrap_or_else(|e| e.into_inner()) = Some(services);
    }

    /// bmcp-identity's services (proxy routing, identities).
    pub fn identities(&self) -> Result<Arc<Services>, CmdError> {
        self.identities
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| {
                CmdError::internal(
                    "identity services missing: call bmcp_identity::install before \
                     bmcp_profiles::install",
                )
            })
    }

    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.profiles_dir.join(id)
    }

    /// The preferences last handed to the browser for `id`.
    pub fn applied(&self, id: &str) -> Option<Prefs> {
        self.lock_applied().get(id).cloned()
    }

    /// Forget what was applied to `id` (the profile is gone).
    pub fn forget_applied(&self, id: &str) {
        self.lock_applied().remove(id);
    }

    fn lock_applied(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Prefs>> {
        self.applied.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set the default of the profile's routing to `proxy` (`None`/"" = direct), keeping its
    /// split rules. Any proxy reference works: a proxy URL, `pool:NAME`, `fwd-N`,
    /// `lease:POOL/RUN/ROW`, `direct`. Nothing is stored when the route cannot work.
    pub async fn set_proxy(&self, id: &str, proxy: Option<&str>) -> Result<(), CmdError> {
        let services = self.identities()?;
        let mut set = services.routes(id)?;
        set.default = proxy
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or("direct")
            .to_string();
        services.set_routes(id, set).await.map_err(|e| {
            e.with_hint(
                "proxy: http://user:pass@host:port, socks5://user:pass@host:port, \
                 host:port:user:pass, pool:NAME (proxy_pool_list), fwd-N or direct.",
            )
        })?;
        Ok(())
    }

    /// Move a proxy stored in `profiles.json` by older versions into the routing.
    async fn migrate_proxy(&self, id: &str, config: &ProfileConfig) -> Result<(), CmdError> {
        let Some(proxy) = config.proxy.as_deref() else {
            return Ok(());
        };
        let services = self.identities()?;
        if services.routes(id)? == RuleSet::default() {
            self.set_proxy(id, Some(proxy)).await?;
        }
        self.store.update(|f| {
            if let Some(c) = f.profiles.get_mut(id) {
                c.proxy = None;
            }
            Ok(())
        })?;
        tracing::info!(profile = id, "proxy moved into the profile's proxy routing");
        Ok(())
    }

    /// Work out the preferences of `id` from its settings, its routing and its identity
    /// (starting the local forwarders its routes need) and hand them to the browser: right
    /// away for its live request contexts, and remembered for the ones created later.
    pub async fn apply(&self, id: &str) -> Result<Prefs, CmdError> {
        let config = self
            .store
            .read()?
            .profiles
            .get(id)
            .cloned()
            .unwrap_or_default();
        self.migrate_proxy(id, &config).await?;
        let services = self.identities()?;
        let proxy = services.profile_proxy_pref(id).await?;
        let identity = services.identity_for_profile(id).unwrap_or_else(|err| {
            tracing::warn!(profile = id, %err, "assigned identity not found");
            None
        });
        let prefs = Prefs::build(
            proxy.pref,
            config.proxy_bypass.as_deref(),
            identity.as_ref(),
            config.accept_language.as_deref(),
        )
        .with_webrtc(config.webrtc);
        if let Ok(backend) = self.backend() {
            // No browser (tests, schema dump): only the files count.
            backend
                .configure(ProfileSettings {
                    id: id.to_string(),
                    prefs: prefs.clone(),
                    label: config.label.clone(),
                })
                .await?;
        }
        self.lock_applied().insert(id.to_string(), prefs.clone());
        Ok(prefs)
    }

    /// Every profile that has settings anywhere: `profiles.json`, proxy routing, an assigned
    /// identity.
    pub fn known_profiles(&self) -> Result<Vec<String>, CmdError> {
        let mut ids: std::collections::BTreeSet<String> =
            self.store.read()?.profiles.into_keys().collect();
        if let Ok(services) = self.identities() {
            ids.extend(services.proxies.read()?.routes.into_keys());
            ids.extend(services.identities.read()?.profiles.into_keys());
        }
        Ok(ids.into_iter().collect())
    }
}

impl bmcp_core::commands::tabs::TabDefaults for ProfilesService {
    fn incognito_default(&self, profile: &str) -> bool {
        match self.store.read() {
            Ok(file) => file
                .profiles
                .get(profile)
                .is_some_and(|c| c.incognito_default),
            Err(err) => {
                // A broken profiles.json must not stop tabs from opening.
                tracing::warn!(%err, "profiles.json unreadable; tab opens normally");
                false
            }
        }
    }
}
