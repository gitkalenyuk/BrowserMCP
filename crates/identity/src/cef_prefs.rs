//! Chromium preferences a profile's `CefRequestContext` needs besides `proxy`
//! (`SetPreference(name, value)` on the UI thread). Pure; the `cef` crate applies them
//! (through `bmcp-profiles`).
//!
//! * WebRTC must not bypass the proxy: without these, STUN over UDP reveals the real IP
//!   (and local addresses) even when every HTTP request goes through the proxy.
//! * `intl.accept_languages` sets Accept-Language at the network level, so requests that do
//!   not go through a tab's DevTools session (service worker fetches, downloads) carry the
//!   identity's languages too.

use serde_json::{Value, json};

use crate::identity::Identity;

/// Preferences for a profile. `proxied`: the profile routes traffic through a proxy (then
/// WebRTC is limited to the proxy; for a direct profile that would only break calls). The
/// WebRTC keys are always present (Chromium's defaults when not proxied), so a profile whose
/// proxy was removed gets its normal WebRTC policy back.
pub fn profile_prefs(identity: Option<&Identity>, proxied: bool) -> Vec<(&'static str, Value)> {
    let mut prefs = vec![
        (
            "webrtc.ip_handling_policy",
            json!(if proxied {
                "disable_non_proxied_udp"
            } else {
                "default"
            }),
        ),
        ("webrtc.multiple_routes_enabled", json!(!proxied)),
        ("webrtc.nonproxied_udp_enabled", json!(!proxied)),
    ];
    if let Some(identity) = identity {
        // Chromium's own format: a comma-separated list without q-values.
        prefs.push(("intl.accept_languages", json!(identity.accept_language)));
    }
    prefs
}

/// Does a profile with this `proxy` preference (see
/// [`ProfileProxy::pref`](crate::ProfileProxy::pref)) send traffic through a proxy?
pub fn is_proxied(pref: &Value) -> bool {
    pref["mode"].as_str().is_some_and(|mode| mode != "direct")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Overrides, generate};

    #[test]
    fn webrtc_limited_when_proxied_languages_only_with_identity() {
        let proxied = profile_prefs(None, true);
        let names: Vec<&str> = proxied.iter().map(|p| p.0).collect();
        assert_eq!(
            names,
            [
                "webrtc.ip_handling_policy",
                "webrtc.multiple_routes_enabled",
                "webrtc.nonproxied_udp_enabled"
            ]
        );
        assert_eq!(proxied[0].1, "disable_non_proxied_udp");
        assert_eq!(proxied[1].1, json!(false));
        assert_eq!(proxied[2].1, json!(false));
        // Direct: the same keys with Chromium's defaults (resets a removed proxy).
        let direct = profile_prefs(None, false);
        assert_eq!(direct[0].1, "default");
        assert_eq!(direct[1].1, json!(true));
        assert_eq!(direct[2].1, json!(true));

        let id = generate(
            None,
            1,
            &Overrides {
                country: Some("DE".into()),
                ..Overrides::default()
            },
        );
        let prefs = profile_prefs(Some(&id), false);
        assert_eq!(prefs.len(), 4);
        assert_eq!(
            prefs[3],
            ("intl.accept_languages", json!("de-DE,de,en-US,en"))
        );
    }

    #[test]
    fn proxied_modes() {
        assert!(is_proxied(&json!({"mode": "pac_script"})));
        assert!(is_proxied(&json!({"mode": "fixed_servers"})));
        assert!(!is_proxied(&json!({"mode": "direct"})));
        assert!(!is_proxied(&json!({})));
    }
}
