//! Chromium preferences of one profile (pure: turned into `CefRequestContext::SetPreference`
//! calls by the CEF crate). The values come from `bmcp-identity`, the one source for them:
//!
//! | Preference | From | Value |
//! |---|---|---|
//! | `proxy` | [`Services::profile_proxy_pref`](bmcp_identity::Services::profile_proxy_pref) (the profile's `proxy.rules_set` routes / `profile.create {proxy}`) | `{"mode": "direct"}`, `fixed_servers` to a local forwarder (+ `bypass_list` from `profile.create {proxy_bypass}`), or `pac_script` |
//! | `webrtc.ip_handling_policy`, `webrtc.multiple_routes_enabled`, `webrtc.nonproxied_udp_enabled` | [`cef_prefs::profile_prefs`](bmcp_identity::cef_prefs::profile_prefs) | limited to the proxy when proxied, Chromium's defaults otherwise |
//! | `intl.accept_languages` | `profile.create {accept_language}`, else the assigned identity's | `de-DE,de,en-US,en` |

use bmcp_identity::Identity;
use bmcp_identity::cef_prefs::{is_proxied, profile_prefs};
use serde_json::{Value, json};

const ACCEPT_LANGUAGES: &str = "intl.accept_languages";

/// What one profile's request context should look like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    /// The `proxy` preference (contains only local addresses: passwords stay in the local
    /// forwarders of bmcp-identity).
    pub proxy: Value,
    /// Accept-Language list without q-values, e.g. `de-DE,de,en-US,en` (also handed to
    /// `RequestContextSettings.accept_language_list` when the context is created). `None` =
    /// Chromium's default.
    pub accept_language: Option<String>,
    /// The other preferences, in order (WebRTC policy, `intl.accept_languages`).
    pub other: Vec<(&'static str, Value)>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs::build(json!({ "mode": "direct" }), None, None, None)
    }
}

impl Prefs {
    /// Put together a profile's preferences: `proxy` from
    /// `Services::profile_proxy_pref(profile).pref`, `bypass` = hosts that skip a fixed proxy
    /// (Chromium syntax, ignored for PAC routing, where `direct` rules do that),
    /// `identity` = the one assigned to the profile, `accept_language` = an explicit list that
    /// wins over the identity's.
    pub fn build(
        mut proxy: Value,
        bypass: Option<&str>,
        identity: Option<&Identity>,
        accept_language: Option<&str>,
    ) -> Prefs {
        if proxy["mode"] == "fixed_servers"
            && let Some(bypass) = bypass.map(str::trim).filter(|b| !b.is_empty())
        {
            proxy["bypass_list"] = json!(bypass);
        }
        let mut other = profile_prefs(identity, is_proxied(&proxy));
        let explicit = accept_language
            .map(strip_q_values)
            .filter(|l| !l.is_empty());
        if let Some(lang) = &explicit {
            other.retain(|(name, _)| *name != ACCEPT_LANGUAGES);
            other.push((ACCEPT_LANGUAGES, json!(lang)));
        }
        let accept_language =
            explicit.or_else(|| identity.map(|i| strip_q_values(&i.accept_language)));
        Prefs {
            proxy,
            accept_language,
            other,
        }
    }

    /// Does the profile's traffic go through a proxy?
    pub fn proxied(&self) -> bool {
        is_proxied(&self.proxy)
    }

    /// `direct`, `fixed_servers` or `pac_script`.
    pub fn proxy_mode(&self) -> &str {
        self.proxy["mode"].as_str().unwrap_or("direct")
    }

    /// The server of a `fixed_servers` proxy (the local forwarder, `socks5://127.0.0.1:p`).
    pub fn proxy_server(&self) -> Option<&str> {
        self.proxy["server"].as_str()
    }

    /// The preferences to set, in order. The proxy and WebRTC keys are always present, so
    /// removing a proxy also resets the WebRTC policy.
    pub fn to_preferences(&self) -> Vec<(&'static str, Value)> {
        let mut prefs = vec![("proxy", self.proxy.clone())];
        prefs.extend(self.other.iter().cloned());
        prefs
    }
}

/// `de-DE,de;q=0.9,en;q=0.8` → `de-DE,de,en` (the pref wants a plain list; Chromium adds
/// the q-values to the header itself).
pub fn strip_q_values(list: &str) -> String {
    list.split(',')
        .map(|item| item.split(';').next().unwrap_or("").trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(prefs: &'a [(&'static str, Value)], name: &str) -> Option<&'a Value> {
        prefs.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    fn identity(country: &str) -> Identity {
        bmcp_identity::generate(
            None,
            7,
            &bmcp_identity::identity::Overrides {
                country: Some(country.into()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn proxy_turns_on_webrtc_protection() {
        let prefs = Prefs::build(
            json!({"mode": "fixed_servers", "server": "socks5://127.0.0.1:5000"}),
            Some("<local>"),
            None,
            Some("de-DE,de;q=0.9,en;q=0.8"),
        );
        let list = prefs.to_preferences();
        assert_eq!(list[0].0, "proxy");
        assert_eq!(
            get(&list, "proxy").unwrap(),
            &json!({"mode": "fixed_servers", "server": "socks5://127.0.0.1:5000", "bypass_list": "<local>"})
        );
        assert_eq!(
            get(&list, "webrtc.ip_handling_policy").unwrap(),
            "disable_non_proxied_udp"
        );
        assert_eq!(
            get(&list, "webrtc.multiple_routes_enabled").unwrap(),
            &json!(false)
        );
        assert_eq!(
            get(&list, "webrtc.nonproxied_udp_enabled").unwrap(),
            &json!(false)
        );
        assert_eq!(get(&list, ACCEPT_LANGUAGES).unwrap(), "de-DE,de,en");
        assert_eq!(prefs.accept_language.as_deref(), Some("de-DE,de,en"));
        assert_eq!(prefs.proxy_server(), Some("socks5://127.0.0.1:5000"));
        assert!(prefs.proxied());
    }

    #[test]
    fn no_proxy_resets_everything() {
        let prefs = Prefs::default();
        let list = prefs.to_preferences();
        assert_eq!(get(&list, "proxy").unwrap(), &json!({"mode": "direct"}));
        assert_eq!(get(&list, "webrtc.ip_handling_policy").unwrap(), "default");
        assert_eq!(
            get(&list, "webrtc.multiple_routes_enabled").unwrap(),
            &json!(true)
        );
        assert!(get(&list, ACCEPT_LANGUAGES).is_none());
        assert!(!prefs.proxied());
        assert_eq!(prefs.proxy_mode(), "direct");
    }

    #[test]
    fn identity_language_unless_explicit_and_no_bypass_for_pac() {
        let fr = identity("FR");
        let pac = json!({"mode": "pac_script", "pac_url": "data:,x", "pac_mandatory": true});
        let prefs = Prefs::build(pac.clone(), Some("<local>"), Some(&fr), None);
        assert_eq!(prefs.proxy, pac, "bypass lists are for fixed proxies only");
        assert!(prefs.proxied());
        let lang = get(&prefs.other, ACCEPT_LANGUAGES)
            .unwrap()
            .as_str()
            .unwrap();
        assert!(lang.starts_with("fr-FR"), "{lang}");
        assert_eq!(prefs.accept_language.as_deref(), Some(lang));
        // An explicit list wins and appears once.
        let prefs = Prefs::build(pac, None, Some(&fr), Some("uk"));
        let langs: Vec<_> = prefs
            .other
            .iter()
            .filter(|(n, _)| *n == ACCEPT_LANGUAGES)
            .collect();
        assert_eq!(langs, [&(ACCEPT_LANGUAGES, json!("uk"))]);
        assert_eq!(prefs.accept_language.as_deref(), Some("uk"));
    }

    #[test]
    fn q_values() {
        assert_eq!(strip_q_values("en-US"), "en-US");
        assert_eq!(strip_q_values(" uk , en;q=0.5,"), "uk,en");
        assert_eq!(strip_q_values(""), "");
    }
}
