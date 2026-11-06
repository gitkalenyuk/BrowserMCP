//! [`Services`] methods the `cef` and `jobs` crates call (no CEF types here):
//!
//! * [`Services::profile_proxy_pref`] — the `proxy` preference for a profile's
//!   `CefRequestContext` (starts the local forwarders it needs);
//! * [`Services::identity_for_profile`] — the identity to auto-apply to a profile's new tabs
//!   and workers;
//! * [`Services::lease`] / [`Services::release`] — one proxy per (run, row) for mass runs;
//! * [`Services::upstream`] — the upstream proxy a reference stands for (geo lookups).
//!
//! When `proxy.rules_set` changes a profile's routing it emits [`ROUTES_CHANGED`] on the
//! event bus with `{profile, pref}`; the `cef` crate applies `pref` to that profile's request
//! context (UI thread) without polling.

use std::net::SocketAddr;

use bmcp_core::{CmdError, ErrorCode, Retry};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::Services;
use crate::identity::Identity;
use crate::proxy::route::{self, Pattern, ProxyRef, RuleSet};
use crate::proxy::{Forwarder, Leased, ProxyConfig};
use crate::rng::Rng;

/// Event topic: a profile's proxy routing changed. Data: `{profile, pref}` (`pref` as in
/// [`ProfileProxy::pref`]; contains only local addresses, no passwords).
pub const ROUTES_CHANGED: &str = "proxy.routes_changed";

/// Event topic: the identity assigned to a profile changed (`identity.assign`, or
/// `identity.remove` of an assigned identity). Data: `{profile, id}` (`id` null = none).
/// `bmcp-profiles` re-applies the profile's preferences (Accept-Language) on it.
pub const IDENTITY_ASSIGNED: &str = "identity.assigned";

/// The run id under which a profile's `pool:NAME` routes lease their proxy, so a profile
/// keeps its IP across restarts.
pub fn profile_run(profile: &str) -> String {
    format!("profile.{profile}")
}

/// A profile's proxy setup, ready for Chromium.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProfileProxy {
    /// Value for `CefRequestContext::SetPreference("proxy", pref)`:
    /// `{"mode": "direct"}`, `{"mode": "fixed_servers", "server": "socks5://127.0.0.1:p"}` or
    /// `{"mode": "pac_script", "pac_url": "data:...", "pac_mandatory": true}`.
    pub pref: Value,
    /// The PAC script (only for `pac_script`).
    pub pac_script: Option<String>,
    /// Local forwarders the routes use ("fwd-1", ...).
    pub forwarders: Vec<String>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn not_found(message: String, hint: &str) -> CmdError {
    CmdError::new(ErrorCode::NotFound, message)
        .with_retry(Retry::No)
        .with_hint(hint)
}

impl Services {
    /// The identity assigned to a profile (`identity.assign`), if any.
    pub fn identity_for_profile(&self, profile: &str) -> Result<Option<Identity>, CmdError> {
        let file = self.identities.read()?;
        match file.profiles.get(profile) {
            Some(id) => file.get(id).map(|i| Some(i.clone())),
            None => Ok(None),
        }
    }

    /// Lease a proxy of `pool` to (run, row) (see [`crate::proxy::pool`]). Returns the
    /// upstream and the lease details.
    pub fn lease(
        &self,
        pool: &str,
        run: &str,
        row: u64,
        rotate: bool,
    ) -> Result<(ProxyConfig, Leased), CmdError> {
        let leased = self.proxies.update(|f| {
            let names: Vec<Value> = f.pools.keys().map(|k| Value::String(k.clone())).collect();
            let p = f.pools.get_mut(pool).ok_or_else(|| {
                not_found(
                    format!("no proxy pool {pool:?}"),
                    "Create it with proxy_pool_add.",
                )
                .with_candidates(names)
            })?;
            p.lease(run, row, rotate, unix_now()).ok_or_else(|| {
                not_found(
                    format!("pool {pool:?} has no healthy proxy"),
                    "Add proxies (proxy_pool_add) or re-check them (proxy_pool_check).",
                )
            })
        })?;
        let proxy = leased
            .url
            .parse::<ProxyConfig>()
            .map_err(|e| CmdError::internal(format!("stored proxy is invalid: {e}")))?;
        Ok((proxy, leased))
    }

    /// Release (run, row), or every lease of the run (`row = None`). Unknown pools release
    /// nothing.
    pub fn release(&self, pool: &str, run: &str, row: Option<u64>) -> Result<usize, CmdError> {
        self.proxies
            .update(|f| Ok(f.pools.get_mut(pool).map_or(0, |p| p.release(run, row))))
    }

    /// The upstream proxy behind a reference (`None` = direct). `holder` is the run id that
    /// keeps `pool:NAME` on one proxy (a lease on row 0); without it the pool's rotation
    /// strategy picks.
    pub fn upstream(
        &self,
        via: &ProxyRef,
        holder: Option<&str>,
    ) -> Result<Option<ProxyConfig>, CmdError> {
        Ok(Some(match via {
            ProxyRef::Direct => return Ok(None),
            ProxyRef::Url(p) => p.clone(),
            ProxyRef::Forwarder(id) => self
                .forwarders()
                .get(id)
                .map(|f| f.upstream().clone())
                .ok_or_else(|| {
                    not_found(
                        format!("no running forwarder {id:?}"),
                        "proxy_forward_list shows the running ones.",
                    )
                })?,
            ProxyRef::Pool(pool) => match holder {
                Some(run) => self.lease(pool, run, 0, false)?.0,
                None => self.pick(pool)?,
            },
            ProxyRef::Lease { pool, run, row } => {
                let file = self.proxies.read()?;
                let url = file
                    .pools
                    .get(pool)
                    .and_then(|p| p.leased(run, *row))
                    .ok_or_else(|| {
                        not_found(
                            format!("no lease for row {row} of run {run:?} in pool {pool:?}"),
                            "Lease one first with proxy_lease.",
                        )
                    })?;
                url.parse::<ProxyConfig>()
                    .map_err(|e| CmdError::internal(format!("stored proxy is invalid: {e}")))?
            }
        }))
    }

    /// Pick from a pool by its rotation strategy.
    fn pick(&self, pool: &str) -> Result<ProxyConfig, CmdError> {
        let url = self.proxies.update(|f| {
            let p = f.pools.get_mut(pool).ok_or_else(|| {
                not_found(
                    format!("no proxy pool {pool:?}"),
                    "Create it with proxy_pool_add.",
                )
            })?;
            p.pick(None, &mut Rng::from_time()).ok_or_else(|| {
                not_found(
                    format!("pool {pool:?} has no healthy proxy"),
                    "Add proxies (proxy_pool_add) or re-check them (proxy_pool_check).",
                )
            })
        })?;
        url.parse::<ProxyConfig>()
            .map_err(|e| CmdError::internal(format!("stored proxy is invalid: {e}")))
    }

    /// A running forwarder to `upstream` (reused when one exists), returning (id, local
    /// address). Chromium then talks plain SOCKS5 to 127.0.0.1.
    pub async fn ensure_forwarder(
        &self,
        upstream: &ProxyConfig,
    ) -> Result<(String, SocketAddr), CmdError> {
        let existing = |s: &Services| {
            s.forwarders()
                .iter()
                .find(|(_, f)| f.upstream() == upstream)
                .map(|(id, f)| (id.clone(), f.local_addr()))
        };
        if let Some(found) = existing(self) {
            return Ok(found);
        }
        let forwarder = Forwarder::start(upstream.clone())
            .await
            .map_err(|e| CmdError::internal(format!("cannot listen on 127.0.0.1: {e}")))?;
        // Another call may have started one meanwhile: keep the first.
        let mut all = self.forwarders();
        if let Some((id, f)) = all.iter().find(|(_, f)| f.upstream() == upstream) {
            return Ok((id.clone(), f.local_addr()));
        }
        let id = self.next_forwarder_id();
        let addr = forwarder.local_addr();
        all.insert(id.clone(), forwarder);
        Ok((id, addr))
    }

    /// The PAC target for a reference: `DIRECT`, or the SOCKS5 address of its forwarder.
    async fn pac_target(
        &self,
        via: &ProxyRef,
        profile: &str,
        used: &mut Vec<String>,
    ) -> Result<String, CmdError> {
        // A running forwarder is used as-is (its upstream may be swapped by rotation).
        if let ProxyRef::Forwarder(id) = via {
            let addr = self.forwarders().get(id).map(|f| f.local_addr());
            let addr = addr.ok_or_else(|| {
                not_found(
                    format!("no running forwarder {id:?}"),
                    "proxy_forward_list shows the running ones.",
                )
            })?;
            used.push(id.clone());
            return Ok(route::pac_target(Some(addr)));
        }
        match self.upstream(via, Some(&profile_run(profile)))? {
            None => Ok(route::pac_target(None)),
            Some(upstream) => {
                let (id, addr) = self.ensure_forwarder(&upstream).await?;
                used.push(id);
                Ok(route::pac_target(Some(addr)))
            }
        }
    }

    /// The `proxy` preference for `profile` from its stored rules (`proxy.rules_set`):
    /// resolves every route to a local forwarder (starting the ones that are missing, e.g.
    /// after a restart). The `cef` crate calls this when it creates the profile's request
    /// context and on [`ROUTES_CHANGED`] events. A profile without rules gets
    /// `{"mode": "direct"}`.
    pub async fn profile_proxy_pref(&self, profile: &str) -> Result<ProfileProxy, CmdError> {
        let set = self
            .proxies
            .read()?
            .routes
            .get(profile)
            .cloned()
            .unwrap_or_default();
        self.proxy_pref_for(profile, &set).await
    }

    /// The stored routing of `profile` (`direct` without rules when it has none).
    pub fn routes(&self, profile: &str) -> Result<RuleSet, CmdError> {
        Ok(self
            .proxies
            .read()?
            .routes
            .get(profile)
            .cloned()
            .unwrap_or_default())
    }

    /// Store a profile's routing (what `proxy.rules_set` does, minus the event): validates and
    /// normalizes the references, resolves every route first (nothing is stored when one
    /// cannot work, e.g. an empty pool), stores it (a direct set without rules removes the
    /// entry) and gives back the profile's pool leases it no longer routes through. Returns the
    /// normalized set and the new preference. Callers that do not apply the preference
    /// themselves emit [`ROUTES_CHANGED`].
    pub async fn set_routes(
        &self,
        profile: &str,
        mut set: RuleSet,
    ) -> Result<(RuleSet, ProfileProxy), CmdError> {
        let parsed = set.parse().map_err(|e| {
            CmdError::invalid_params(e).with_hint(
                "Rules: {match, via}; via/default: direct, fwd-N, pool:NAME, \
                 lease:POOL/RUN/ROW or a proxy URL.",
            )
        })?;
        // Store references in their normalized form (proxy URLs as parsed).
        for (rule, (_, via)) in set.rules.iter_mut().zip(&parsed.rules) {
            rule.via = via.to_stored();
        }
        set.default = parsed.default.to_stored();
        let proxy = self.proxy_pref_for(profile, &set).await?;
        let pools: Vec<&str> = parsed
            .rules
            .iter()
            .map(|(_, v)| v)
            .chain([&parsed.default])
            .filter_map(|v| match v {
                ProxyRef::Pool(p) => Some(p.as_str()),
                _ => None,
            })
            .collect();
        let holder = profile_run(profile);
        self.proxies.update(|f| {
            if set.is_direct() {
                f.routes.remove(profile);
            } else {
                f.routes.insert(profile.to_string(), set.clone());
            }
            // Pools the profile no longer routes through give its proxy back.
            for (name, pool) in f.pools.iter_mut() {
                if !pools.contains(&name.as_str()) {
                    pool.release(&holder, None);
                }
            }
            Ok(())
        })?;
        Ok((set, proxy))
    }

    /// A fresh identity for `profile` (deterministic by `seed`), fitted to the exit IP of
    /// `via` when given (geo lookup through the proxy at `geo_url`, default ip-api.com; what
    /// `identity.fit_to_proxy` does), saved under the id `profile` and assigned to the
    /// profile. Used for the isolated rows of mass runs (`run.start {fit_identity}`).
    pub async fn identity_for_new_profile(
        &self,
        profile: &str,
        seed: u64,
        via: Option<&ProxyRef>,
        geo_url: Option<&str>,
    ) -> Result<Identity, CmdError> {
        let mut identity = crate::identity::generate(None, seed, &Default::default());
        identity.id = profile.to_string();
        if let Some(via) = via
            && let Some(upstream) = self.upstream(via, Some(&profile_run(profile)))?
        {
            let url = geo_url.unwrap_or(crate::proxy::geo::DEFAULT_URL);
            let info =
                crate::proxy::geo::lookup(Some(&upstream), url, std::time::Duration::from_secs(10))
                    .await
                    .map_err(|e| {
                        CmdError::new(ErrorCode::Internal, format!("geo lookup failed: {e}"))
                            .with_retry(Retry::AfterWait)
                    })?;
            crate::fit::fit_to_geo(&mut identity, &info);
        }
        let copy = identity.clone();
        self.identities.update(|f| {
            f.identities.insert(copy.id.clone(), copy);
            f.profiles.insert(profile.to_string(), profile.to_string());
            Ok(())
        })?;
        Ok(identity)
    }

    /// Forget what this crate keeps for a profile that goes away: its routes (and the pool
    /// leases they held) and its identity assignment. With `own_identity`, also the identity
    /// saved under the profile's own id when it is the assigned one (what
    /// [`Self::identity_for_new_profile`] made); other identities always stay.
    pub fn forget_profile(&self, profile: &str, own_identity: bool) -> Result<(), CmdError> {
        let holder = profile_run(profile);
        self.proxies.update(|f| {
            f.routes.remove(profile);
            for pool in f.pools.values_mut() {
                pool.release(&holder, None);
            }
            Ok(())
        })?;
        self.identities.update(|f| {
            if f.profiles.remove(profile).as_deref() == Some(profile) && own_identity {
                f.identities.remove(profile);
            }
            Ok(())
        })
    }

    /// [`Self::profile_proxy_pref`] for a rule set that is not stored (yet).
    pub async fn proxy_pref_for(
        &self,
        profile: &str,
        set: &RuleSet,
    ) -> Result<ProfileProxy, CmdError> {
        let parsed = set
            .parse()
            .map_err(|e| CmdError::invalid_params(format!("rules of {profile:?}: {e}")))?;
        let mut used = Vec::new();
        let mut rules: Vec<(Pattern, String)> = Vec::new();
        for (pattern, via) in &parsed.rules {
            let target = self.pac_target(via, profile, &mut used).await?;
            rules.push((pattern.clone(), target));
        }
        let default = self.pac_target(&parsed.default, profile, &mut used).await?;
        used.sort();
        used.dedup();
        let pref = route::chromium_pref(&rules, &default);
        let pac_script =
            (pref["mode"] == "pac_script").then(|| route::pac_script(&rules, &default));
        Ok(ProfileProxy {
            pref,
            pac_script,
            forwarders: used,
        })
    }
}
