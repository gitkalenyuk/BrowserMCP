//! Proxies: URL parsing ([`ProxyConfig`]), pools with rotation, leases and health checks, a
//! local forwarder that adds authentication for Chromium, split routing (PAC) and the IP → geo
//! lookup.
//!
//! CEF wiring (not done here; the `cef` crate owns it): give each profile its own
//! `CefRequestContext` and set its `proxy` preference to
//! [`Services::profile_proxy_pref`](crate::Services::profile_proxy_pref) (a fixed local
//! forwarder or a PAC script), plus the WebRTC/language prefs from
//! [`crate::cef_prefs::profile_prefs`]. See this crate's README.

pub mod config;
pub mod forward;
pub mod geo;
pub mod pool;
pub mod route;

pub use config::{ProxyConfig, Scheme};
pub use forward::Forwarder;
pub use geo::GeoInfo;
pub use pool::{Lease, Leased, Pool, ProxiesFile, ProxyEntry, Strategy};
pub use route::{ProxyRef, Rule, RuleSet};
