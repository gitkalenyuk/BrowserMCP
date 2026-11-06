//! Isolated profiles of mass-run rows with a leased proxy and, optionally, an identity fitted
//! to it (`run.start {profile: "isolated", proxy_pool, fit_identity}`).
//!
//! Per row attempt (the jobs crate calls this through its row-profile hook; the app connects
//! the two):
//!
//! 1. [`RowProfiles::prepare`]: `proxy.lease {pool, run, row, rotate}` (sticky per row; a
//!    retry rotates to another proxy) → the attempt's in-memory profile is routed through
//!    `lease:POOL/RUN/ROW` (bmcp-identity routing, so its forwarder and preference come from
//!    the same place as for any other profile) → with `fit_identity` a fresh identity is
//!    generated, fitted to the proxy's exit IP and assigned → the profile's preferences go to
//!    the browser. Its tabs then get the identity like any identity profile
//!    ([`crate::IdentityPreparer`]).
//! 2. [`RowProfiles::release`]: the attempt's profile is forgotten (routing, identity,
//!    browser settings); at the end of the row also the lease.
//! 3. [`RowProfiles::finish_run`]: every lease of the run is released and leftovers of the
//!    run's row profiles (e.g. from a restart in the middle) are removed.

use std::sync::Arc;

use bmcp_core::{CmdError, ErrorCode, Retry};
use bmcp_identity::proxy::RuleSet;
use bmcp_identity::proxy::route::{ProxyRef, lease_ref};

use crate::service::ProfilesService;

/// One row attempt's profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowRequest {
    pub run: String,
    pub row: u64,
    /// The attempt's (in-memory) profile id, as the script host gets it in its tab defaults.
    pub profile: String,
    /// Lease a proxy of this pool for the row.
    pub pool: Option<String>,
    /// Generate an identity fitted to the row's proxy.
    pub fit_identity: bool,
    /// A retry: give the row another proxy than it had.
    pub rotate: bool,
}

/// What a row attempt got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSetup {
    /// The leased proxy, password masked.
    pub proxy: Option<String>,
    /// `lease:POOL/RUN/ROW`.
    pub proxy_id: Option<String>,
    /// The generated identity's id (= the profile id).
    pub identity: Option<String>,
}

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub struct RowProfiles {
    service: Arc<ProfilesService>,
    /// Geo lookup endpoint for `fit_identity` (default: ip-api.com).
    geo_url: Option<String>,
}

impl RowProfiles {
    pub fn new(service: Arc<ProfilesService>) -> Self {
        RowProfiles {
            service,
            geo_url: None,
        }
    }

    /// Use another geo lookup endpoint (ip-api.com's JSON format; tests).
    pub fn with_geo_url(mut self, url: impl Into<String>) -> Self {
        self.geo_url = Some(url.into());
        self
    }

    /// Validate `run.start {proxy_pool, fit_identity}` before the run starts.
    pub fn check(&self, pool: Option<&str>, fit_identity: bool) -> Result<(), CmdError> {
        let Some(pool) = pool else {
            if fit_identity {
                return Err(CmdError::invalid_params(
                    "fit_identity needs proxy_pool (the identity is fitted to the row's proxy)",
                ));
            }
            return Ok(());
        };
        let services = self.service.identities()?;
        let file = services.proxies.read()?;
        match file.pools.get(pool) {
            Some(p) if !p.proxies.is_empty() => Ok(()),
            Some(_) => Err(
                CmdError::invalid_params(format!("proxy pool {pool:?} is empty"))
                    .with_hint("Add proxies with proxy_pool_add."),
            ),
            None => Err(
                CmdError::new(ErrorCode::NotFound, format!("no proxy pool {pool:?}"))
                    .with_retry(Retry::No)
                    .with_hint("Create it with proxy_pool_add (proxy_pool_list shows the pools).")
                    .with_candidates(
                        file.pools
                            .keys()
                            .map(|k| serde_json::Value::String(k.clone()))
                            .collect(),
                    ),
            ),
        }
    }

    /// Step 1 of the module docs. On an error the attempt's profile is cleaned up again
    /// (the lease stays, so a retry can rotate it).
    pub async fn prepare(&self, req: &RowRequest) -> Result<RowSetup, CmdError> {
        let result = self.try_prepare(req).await;
        if result.is_err() {
            self.forget_profile(&req.profile).await;
        }
        result
    }

    async fn try_prepare(&self, req: &RowRequest) -> Result<RowSetup, CmdError> {
        let services = self.service.identities()?;
        let mut setup = RowSetup {
            proxy: None,
            proxy_id: None,
            identity: None,
        };
        let mut via = None;
        if let Some(pool) = &req.pool {
            let (proxy, leased) = services.lease(pool, &req.run, req.row, req.rotate)?;
            let proxy_id = lease_ref(pool, &req.run, req.row);
            tracing::debug!(
                run = req.run,
                row = req.row,
                proxy = proxy.masked(),
                reused = leased.reused,
                "row proxy leased"
            );
            services
                .set_routes(
                    &req.profile,
                    RuleSet {
                        rules: Vec::new(),
                        default: proxy_id.clone(),
                    },
                )
                .await?;
            via = Some(ProxyRef::Lease {
                pool: pool.clone(),
                run: req.run.clone(),
                row: req.row,
            });
            setup.proxy = Some(proxy.masked());
            setup.proxy_id = Some(proxy_id);
        }
        if req.fit_identity {
            let identity = services
                .identity_for_new_profile(
                    &req.profile,
                    row_seed(&req.run, req.row),
                    via.as_ref(),
                    self.geo_url.as_deref(),
                )
                .await?;
            setup.identity = Some(identity.id);
        }
        self.service.apply(&req.profile).await?;
        Ok(setup)
    }

    /// Step 2 of the module docs: forget the attempt's profile; `row_done` = the row is over,
    /// give its proxy back too.
    pub async fn release(&self, req: &RowRequest, row_done: bool) {
        self.forget_profile(&req.profile).await;
        if row_done
            && let Some(pool) = &req.pool
            && let Ok(services) = self.service.identities()
            && let Err(err) = services.release(pool, &req.run, Some(req.row))
        {
            tracing::warn!(run = req.run, row = req.row, %err, "proxy lease not released");
        }
    }

    /// Step 3 of the module docs. `profile_prefix` = what the run's row profile ids start
    /// with.
    pub async fn finish_run(&self, run: &str, pool: Option<&str>, profile_prefix: &str) {
        let Ok(services) = self.service.identities() else {
            return;
        };
        if let Some(pool) = pool
            && let Err(err) = services.release(pool, run, None)
        {
            tracing::warn!(run, %err, "proxy leases of the run not released");
        }
        let leftovers: Vec<String> = self
            .service
            .known_profiles()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.starts_with(profile_prefix))
            .collect();
        for profile in leftovers {
            self.forget_profile(&profile).await;
        }
    }

    /// Remove a row profile everywhere: routing (+ its pool holds) and generated identity in
    /// bmcp-identity, the applied preferences here, the settings in the browser (best effort:
    /// the browser refuses while the profile still has tabs; its in-memory context goes away
    /// with the last tab anyway).
    async fn forget_profile(&self, profile: &str) {
        if let Ok(services) = self.service.identities()
            && let Err(err) = services.forget_profile(profile, true)
        {
            tracing::warn!(profile, %err, "row profile not forgotten");
        }
        self.service.forget_applied(profile);
        if let Ok(backend) = self.service.backend()
            && let Err(err) = backend.forget(profile.to_string()).await
        {
            tracing::debug!(profile, %err, "browser kept the row profile");
        }
    }
}

/// Same device for every attempt of a row (FNV-1a of run and row).
fn row_seed(run: &str, row: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in run.bytes().chain(row.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
