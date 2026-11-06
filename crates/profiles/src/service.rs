//! [`ProfilesService`]: stored profile settings (`data/config/profiles.json`), running proxy
//! forwarders, and the link to the browser ([`ProfileBackend`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use bmcp_core::{CmdError, Ctx, ErrorCode};
use bmcp_identity::proxy::Scheme;
use bmcp_identity::store::JsonStore;
use bmcp_identity::{Forwarder, ProxyConfig};
use serde::{Deserialize, Serialize};

use crate::backend::{ProfileBackend, ProfileSettings, ProxyAuth};
use crate::prefs::Prefs;

/// Settings of one profile as stored on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Upstream proxy URL (`socks5://user:pass@host:port`, `http://…`, `host:port:user:pass`).
    /// Stored with its password as plain text, like `proxies.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Go through the local forwarder (default true). `false` = Chromium talks to the proxy
    /// itself (HTTP proxies only; the password is answered in `GetAuthCredentials`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<bool>,
    /// Hosts that skip the proxy (Chromium syntax, `;`-separated, e.g. `<local>;*.lan`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_bypass: Option<String>,
    /// Accept-Language list; default: the assigned identity's, else Chromium's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_language: Option<String>,
    #[serde(default)]
    pub created_ms: i64,
}

/// Contents of `profiles.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilesFile {
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

/// How a profile reaches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyPlan {
    Direct,
    /// Through a local forwarder chained to this upstream.
    Forward(ProxyConfig),
    /// Chromium uses `server` itself, answering proxy auth with `auth`.
    Fixed {
        server: String,
        auth: Option<ProxyAuth>,
    },
}

/// Decide how a profile's proxy is used (pure).
pub fn plan_proxy(config: &ProfileConfig) -> Result<ProxyPlan, CmdError> {
    let Some(url) = config.proxy.as_deref().filter(|u| !u.trim().is_empty()) else {
        return Ok(ProxyPlan::Direct);
    };
    let proxy: ProxyConfig =
        url.parse()
            .map_err(|e: bmcp_identity::proxy::config::ParseError| {
                CmdError::invalid_params(format!("proxy: {e}")).with_hint(
            "Use http://user:pass@host:port, socks5://user:pass@host:port or host:port:user:pass.",
        )
            })?;
    if config.forward.unwrap_or(true) {
        return Ok(ProxyPlan::Forward(proxy));
    }
    if proxy.scheme == Scheme::Socks5 && proxy.has_auth() {
        return Err(CmdError::invalid_params(
            "Chromium cannot log in to a SOCKS5 proxy: keep forward: true (the default)",
        ));
    }
    let server = format!("{}://{}", proxy.scheme.as_str(), proxy.host_port());
    let auth = proxy.username.clone().map(|username| ProxyAuth {
        username,
        password: proxy.password.clone().unwrap_or_default(),
    });
    Ok(ProxyPlan::Fixed { server, auth })
}

/// Shared state of the profile commands (in the registry's extensions).
pub struct ProfilesService {
    pub store: JsonStore<ProfilesFile>,
    /// `data/profiles`: one cache folder per profile.
    profiles_dir: PathBuf,
    /// The profile the browser started with (`--profile`, normally `default`).
    pub launch_profile: String,
    /// Running forwarders by profile id.
    forwarders: Mutex<BTreeMap<String, Forwarder>>,
    backend: RwLock<Option<Arc<dyn ProfileBackend>>>,
}

impl ProfilesService {
    /// `data_root` = the data folder (`config/profiles.json`, `profiles/<id>/`).
    pub fn new(data_root: &std::path::Path, launch_profile: &str) -> Self {
        ProfilesService {
            store: JsonStore::new(data_root.join("config").join("profiles.json")),
            profiles_dir: data_root.join("profiles"),
            launch_profile: launch_profile.to_string(),
            forwarders: Mutex::new(BTreeMap::new()),
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

    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.profiles_dir.join(id)
    }

    /// Local address of the profile's forwarder (e.g. `socks5://127.0.0.1:51234`).
    pub fn forwarder_url(&self, id: &str) -> Option<String> {
        self.lock_forwarders().get(id).map(Forwarder::socks5_url)
    }

    fn lock_forwarders(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Forwarder>> {
        self.forwarders.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stop the profile's forwarder (if any).
    pub fn stop_forwarder(&self, id: &str) {
        let removed = self.lock_forwarders().remove(id);
        drop(removed); // dropping a Forwarder stops it
    }

    /// Work out the settings of `id` (starting/replacing its forwarder) and hand them to the
    /// browser. `identity_language` = Accept-Language of the identity assigned to it.
    pub async fn apply(&self, id: &str, identity_language: Option<String>) -> Result<(), CmdError> {
        let config = self
            .store
            .read()?
            .profiles
            .get(id)
            .cloned()
            .unwrap_or_default();
        let (server, auth) = match plan_proxy(&config)? {
            ProxyPlan::Direct => {
                self.stop_forwarder(id);
                (None, None)
            }
            ProxyPlan::Fixed { server, auth } => {
                self.stop_forwarder(id);
                (Some(server), auth)
            }
            ProxyPlan::Forward(upstream) => {
                // Reuse a running forwarder to the same upstream (its port stays the same).
                let existing = self
                    .lock_forwarders()
                    .get(id)
                    .filter(|f| *f.upstream() == upstream)
                    .map(Forwarder::socks5_url);
                let url = match existing {
                    Some(url) => url,
                    None => {
                        let forwarder = Forwarder::start(upstream).await.map_err(|e| {
                            CmdError::internal(format!("cannot start the proxy forwarder: {e}"))
                        })?;
                        let url = forwarder.socks5_url();
                        self.lock_forwarders().insert(id.to_string(), forwarder);
                        url
                    }
                };
                (Some(url), None)
            }
        };
        let settings = ProfileSettings {
            id: id.to_string(),
            prefs: Prefs {
                proxy_server: server,
                proxy_bypass: config.proxy_bypass.clone(),
                accept_language: config.accept_language.clone().or(identity_language),
            },
            proxy_auth: auth,
        };
        match self.backend() {
            Ok(backend) => backend.configure(settings).await.map_err(CmdError::from),
            Err(_) => Ok(()), // no browser (tests, schema dump): the file is what counts
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(proxy: &str, forward: Option<bool>) -> ProfileConfig {
        ProfileConfig {
            proxy: Some(proxy.into()),
            forward,
            ..Default::default()
        }
    }

    #[test]
    fn proxy_plans() {
        assert_eq!(
            plan_proxy(&ProfileConfig::default()).unwrap(),
            ProxyPlan::Direct
        );
        assert!(matches!(
            plan_proxy(&config("socks5://u:p@h:1080", None)).unwrap(),
            ProxyPlan::Forward(_)
        ));
        assert_eq!(
            plan_proxy(&config("http://u:p@h:3128", Some(false))).unwrap(),
            ProxyPlan::Fixed {
                server: "http://h:3128".into(),
                auth: Some(ProxyAuth {
                    username: "u".into(),
                    password: "p".into()
                })
            }
        );
        assert_eq!(
            plan_proxy(&config("h:8080", Some(false))).unwrap(),
            ProxyPlan::Fixed {
                server: "http://h:8080".into(),
                auth: None
            }
        );
        assert!(plan_proxy(&config("socks5://u:p@h:1", Some(false))).is_err());
        assert!(plan_proxy(&config("ftp://nope", None)).is_err());
    }
}
