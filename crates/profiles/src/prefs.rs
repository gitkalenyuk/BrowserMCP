//! Chromium preferences of one profile (pure: turned into `CefRequestContext::SetPreference`
//! calls by the CEF crate).
//!
//! | Preference | Value | Why |
//! |---|---|---|
//! | `proxy` | `{"mode": "fixed_servers", "server": "socks5://127.0.0.1:<port>"}` or `{"mode": "direct"}` | per-profile proxy (through our local forwarder) |
//! | `webrtc.ip_handling_policy` | `disable_non_proxied_udp` with a proxy, else `default` | WebRTC must not reveal the real/local IP |
//! | `webrtc.multiple_routes_enabled` | `false` with a proxy | same |
//! | `webrtc.nonproxied_udp_enabled` | `false` with a proxy | same |
//! | `intl.accept_languages` | `de-DE,de,en-US,en` | Accept-Language for requests outside a tab's DevTools session (service workers) |

use serde_json::{Value, json};

/// What one profile's request context should look like.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    /// Proxy server for Chromium, e.g. `socks5://127.0.0.1:51234`. `None` = direct.
    pub proxy_server: Option<String>,
    /// Hosts that skip the proxy (`;`-separated, Chromium syntax). Only with a proxy.
    pub proxy_bypass: Option<String>,
    /// Accept-Language list, e.g. `de-DE,de,en-US,en`. `None` = leave Chromium's default.
    pub accept_language: Option<String>,
}

impl Prefs {
    /// The preferences to set, in order. Every key is always present (except the language)
    /// so that removing a proxy also resets the WebRTC policy.
    pub fn to_preferences(&self) -> Vec<(&'static str, Value)> {
        let mut prefs = Vec::new();
        let proxied = self.proxy_server.is_some();
        let proxy = match &self.proxy_server {
            Some(server) => {
                let mut p = json!({ "mode": "fixed_servers", "server": server });
                if let Some(bypass) = self.proxy_bypass.as_deref().filter(|b| !b.is_empty()) {
                    p["bypass_list"] = json!(bypass);
                }
                p
            }
            None => json!({ "mode": "direct" }),
        };
        prefs.push(("proxy", proxy));
        prefs.push((
            "webrtc.ip_handling_policy",
            json!(if proxied {
                "disable_non_proxied_udp"
            } else {
                "default"
            }),
        ));
        prefs.push(("webrtc.multiple_routes_enabled", json!(!proxied)));
        prefs.push(("webrtc.nonproxied_udp_enabled", json!(!proxied)));
        if let Some(lang) = self
            .accept_language
            .as_deref()
            .map(strip_q_values)
            .filter(|l| !l.is_empty())
        {
            prefs.push(("intl.accept_languages", json!(lang)));
        }
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

    #[test]
    fn proxy_turns_on_webrtc_protection() {
        let prefs = Prefs {
            proxy_server: Some("socks5://127.0.0.1:5000".into()),
            proxy_bypass: Some("<local>".into()),
            accept_language: Some("de-DE,de;q=0.9,en;q=0.8".into()),
        }
        .to_preferences();
        assert_eq!(
            get(&prefs, "proxy").unwrap(),
            &json!({"mode": "fixed_servers", "server": "socks5://127.0.0.1:5000", "bypass_list": "<local>"})
        );
        assert_eq!(
            get(&prefs, "webrtc.ip_handling_policy").unwrap(),
            "disable_non_proxied_udp"
        );
        assert_eq!(
            get(&prefs, "webrtc.multiple_routes_enabled").unwrap(),
            &json!(false)
        );
        assert_eq!(
            get(&prefs, "webrtc.nonproxied_udp_enabled").unwrap(),
            &json!(false)
        );
        assert_eq!(get(&prefs, "intl.accept_languages").unwrap(), "de-DE,de,en");
    }

    #[test]
    fn no_proxy_resets_everything() {
        let prefs = Prefs::default().to_preferences();
        assert_eq!(get(&prefs, "proxy").unwrap(), &json!({"mode": "direct"}));
        assert_eq!(get(&prefs, "webrtc.ip_handling_policy").unwrap(), "default");
        assert_eq!(
            get(&prefs, "webrtc.multiple_routes_enabled").unwrap(),
            &json!(true)
        );
        assert!(get(&prefs, "intl.accept_languages").is_none());
    }

    #[test]
    fn q_values() {
        assert_eq!(strip_q_values("en-US"), "en-US");
        assert_eq!(strip_q_values(" uk , en;q=0.5,"), "uk,en");
        assert_eq!(strip_q_values(""), "");
    }
}
