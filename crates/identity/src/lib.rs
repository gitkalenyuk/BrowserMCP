//! BrowserMCP hidden mode, part 1 (Phase 10 foundations): identities, stealth and proxies.
//!
//! * [`identity`] — consistent fingerprint presets and a seeded generator (`identity.*`);
//! * [`stealth`] — apply an identity to a tab through CDP Emulation + a minimal patch script,
//!   remove it, and probe the page like a fingerprinting script would (`stealth.*`,
//!   `device.*`);
//! * [`proxy`] — proxy URLs, pools with rotation, leases and health checks, a local forwarder
//!   that adds SOCKS5/HTTP authentication for Chromium, split routing as a PAC script, and
//!   IP → geo lookup (`proxy.*`, `geo.*`);
//! * [`fit`] + [`country`] — fit an identity's timezone/languages/position to a proxy's exit
//!   IP, with an offline country table (`identity.fit_to_proxy`);
//! * [`worker`] — the CDP commands that give worker targets the identity;
//! * [`routing`] + [`cef_prefs`] — what the `cef` crate calls per profile (proxy preference,
//!   WebRTC/language prefs, the assigned identity) and the leases the `jobs` crate uses.
//!
//! All commands are in [`Toolset::Stealth`](bmcp_core::Toolset::Stealth). State that lives
//! between calls is one [`Services`] value in the registry's extensions. The CEF-side wiring
//! (per-profile proxy preference, WebRTC policy, auto-apply on new tabs and workers) is
//! described in this crate's README.

pub mod cef_prefs;
pub mod commands;
pub mod country;
pub mod fit;
pub mod identity;
pub mod proxy;
mod rng;
pub mod routing;
pub mod stealth;
pub mod store;
#[cfg(test)]
mod tests;
pub mod worker;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bmcp_core::{CmdError, Ctx, Registry};

pub use identity::{Identity, Os, PRESETS, generate};
pub use proxy::{Forwarder, ProxyConfig, ProxyRef};
pub use routing::{ProfileProxy, ROUTES_CHANGED};
pub use worker::{WorkerCommand, worker_commands};

use proxy::ProxiesFile;
use stealth::TabIdentities;
use store::{IdentitiesFile, JsonStore};

/// Long-lived state of this crate (in the registry's extensions).
pub struct Services {
    /// `data/config/identities.json`.
    pub identities: JsonStore<IdentitiesFile>,
    /// `data/config/proxies.json`.
    pub proxies: JsonStore<ProxiesFile>,
    /// What is applied to which tab.
    pub tabs: TabIdentities,
    /// Running local forwarders by id ("fwd-1").
    pub forwarders: Mutex<BTreeMap<String, Forwarder>>,
    next_forwarder: AtomicU64,
}

impl Services {
    /// State that keeps its files in `config_dir` (normally `data/config`).
    pub fn new(config_dir: PathBuf) -> Self {
        Services {
            identities: JsonStore::new(config_dir.join("identities.json")),
            proxies: JsonStore::new(config_dir.join("proxies.json")),
            tabs: TabIdentities::default(),
            forwarders: Mutex::new(BTreeMap::new()),
            next_forwarder: AtomicU64::new(1),
        }
    }

    /// The instance from the call's context.
    pub fn from_ctx(ctx: &Ctx) -> Result<Arc<Services>, CmdError> {
        ctx.extensions.get::<Services>().ok_or_else(|| {
            CmdError::internal("identity services missing: bmcp_identity::register was not called")
        })
    }

    fn next_forwarder_id(&self) -> String {
        format!(
            "fwd-{}",
            self.next_forwarder.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn forwarders(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Forwarder>> {
        self.forwarders.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Give the commands their storage: identities and proxy pools live in `config_dir`
/// (normally `data/config`). Called by the app once the data folder is known; without it the
/// commands fail with a clear error (e.g. inside `bmcp-schema`, which only lists them).
pub fn install(registry: &Registry, config_dir: impl Into<PathBuf>) {
    registry
        .extensions()
        .insert(Services::new(config_dir.into()));
}

/// Register the commands (no storage yet: see [`install`]); identities and proxy pools are stored in `config_dir`
/// (`data/config`).
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::Presets>()
        .add::<commands::Generate>()
        .add::<commands::List>()
        .add::<commands::Get>()
        .add::<commands::Remove>()
        .add::<commands::Assign>()
        .add::<commands::FingerprintSet>()
        .add::<commands::FingerprintClear>()
        .add::<commands::Check>()
        .add::<commands::DeviceEmulate>()
        .add::<commands::DeviceClear>()
        .add::<commands::PoolAdd>()
        .add::<commands::PoolList>()
        .add::<commands::PoolCheck>()
        .add::<commands::PoolRemove>()
        .add::<commands::Next>()
        .add::<commands::ForwardStart>()
        .add::<commands::ForwardStop>()
        .add::<commands::ForwardList>()
        .add::<commands::GeoLookup>()
        .add::<commands::RulesSet>()
        .add::<commands::RulesTest>()
        .add::<commands::FitToProxy>()
        .add::<commands::Lease>()
        .add::<commands::Release>();
}
