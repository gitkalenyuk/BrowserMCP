//! Chromium preferences a profile's `CefRequestContext` needs besides `proxy`
//! (`SetPreference(name, value)` on the UI thread). Pure; the `cef` crate applies them.
//!
//! * WebRTC must not bypass the proxy: without these, STUN over UDP reveals the real IP
//!   (and local addresses) even when every HTTP request goes through the proxy.
//! * `intl.accept_languages` sets Accept-Language at the network level, so requests that do
//!   not go through a tab's DevTools session (service worker fetches, downloads) carry the
//!   identity's languages too.

use serde_json::{Value, json};

use crate::identity::Identity;

/// Preferences for a profile. `proxied`: the profile routes traffic through a proxy (then
/// WebRTC is limited to the proxy; for a direct profile that would only break calls).
pub fn profile_prefs(identity: Option<&Identity>, proxied: bool) -> Vec<(&'static str, Value)> {
    let mut prefs = Vec::new();
    if proxied {
        prefs.push((
            "webrtc.ip_handling_policy",
            json!("disable_non_proxied_udp"),
        ));
        prefs.push(("webrtc.multiple_routes_enabled", json!(false)));
        prefs.push(("webrtc.nonproxied_udp_enabled", json!(false)));
    }
    if let Some(identity) = identity {
        // Chromium's own format: a comma-separated list without q-values.
        prefs.push(("intl.accept_languages", json!(identity.accept_language)));
    }
    prefs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Overrides, generate};

    #[test]
    fn webrtc_only_when_proxied_languages_only_with_identity() {
        assert!(profile_prefs(None, false).is_empty());
        let names: Vec<&str> = profile_prefs(None, true).iter().map(|p| p.0).collect();
        assert_eq!(
            names,
            [
                "webrtc.ip_handling_policy",
                "webrtc.multiple_routes_enabled",
                "webrtc.nonproxied_udp_enabled"
            ]
        );
        let id = generate(
            None,
            1,
            &Overrides {
                country: Some("DE".into()),
                ..Overrides::default()
            },
        );
        let prefs = profile_prefs(Some(&id), false);
        assert_eq!(
            prefs,
            [("intl.accept_languages", json!("de-DE,de,en-US,en"))]
        );
    }
}
