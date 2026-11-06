//! Proxies: URL parsing ([`ProxyConfig`]), pools with rotation and health checks, a local
//! forwarder that adds authentication for Chromium, and the IP → geo lookup.
//!
//! CEF wiring (not done here; the `cef` crate owns it): give each profile its own
//! `CefRequestContext` and call
//! `SetPreference("proxy", {"mode": "fixed_servers", "server": "socks5://127.0.0.1:<port>"})`
//! with the forwarder's address, plus `SetPreference("webrtc.ip_handling_policy",
//! "disable_non_proxied_udp")` so WebRTC cannot reveal the real IP. See this crate's README.

pub mod config;
pub mod forward;
pub mod geo;
pub mod pool;

pub use config::{ProxyConfig, Scheme};
pub use forward::Forwarder;
pub use geo::GeoInfo;
pub use pool::{Pool, ProxiesFile, ProxyEntry, Strategy};
