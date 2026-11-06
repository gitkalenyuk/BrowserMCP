//! Cookies as the commands see them, and the pure conversions around CEF's cookie manager.
//!
//! CEF stores times as `cef_basetime_t`: microseconds since 1601-01-01 (Windows epoch). We
//! use Unix milliseconds everywhere else.

use bmcp_data::import::PendingCookie;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Microseconds between 1601-01-01 and 1970-01-01.
const WINDOWS_TO_UNIX_US: i64 = 11_644_473_600_000_000;

/// One cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    /// `example.com` (host-only) or `.example.com` (also subdomains).
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    /// Unix ms; `null` = session cookie.
    #[serde(default)]
    pub expires: Option<i64>,
    /// `unspecified`, `no_restriction`, `lax` or `strict`.
    #[serde(default = "unspecified")]
    pub same_site: String,
}

fn root_path() -> String {
    "/".into()
}

fn unspecified() -> String {
    "unspecified".into()
}

impl From<&PendingCookie> for CookieInfo {
    fn from(c: &PendingCookie) -> Self {
        CookieInfo {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: if c.path.is_empty() {
                root_path()
            } else {
                c.path.clone()
            },
            secure: c.secure,
            http_only: c.http_only,
            expires: c.expires,
            same_site: normalize_same_site(&c.same_site).to_string(),
        }
    }
}

/// Our spelling of a SameSite value (also accepts Chrome/extension spellings).
pub fn normalize_same_site(value: &str) -> &'static str {
    match value.to_ascii_lowercase().replace('-', "_").as_str() {
        "no_restriction" | "none" => "no_restriction",
        "lax" => "lax",
        "strict" => "strict",
        _ => "unspecified",
    }
}

/// Unix ms → CEF basetime (µs since 1601).
pub fn unix_ms_to_cef(ms: i64) -> i64 {
    ms.saturating_mul(1000).saturating_add(WINDOWS_TO_UNIX_US)
}

/// CEF basetime → Unix ms (0 and negative stay "unknown").
pub fn cef_to_unix_ms(basetime: i64) -> Option<i64> {
    (basetime > 0).then(|| (basetime - WINDOWS_TO_UNIX_US) / 1000)
}

/// The URL CEF's `SetCookie` wants for a cookie: `https://<host><path>` (`http` when the
/// cookie is not secure). The leading dot of a domain cookie is dropped.
pub fn cookie_url(cookie: &CookieInfo) -> String {
    let host = cookie.domain.trim_start_matches('.');
    let scheme = if cookie.secure { "https" } else { "http" };
    let path = if cookie.path.starts_with('/') {
        cookie.path.as_str()
    } else {
        "/"
    };
    format!("{scheme}://{host}{path}")
}

/// The `domain` field for CEF's `SetCookie`: a domain cookie keeps its leading dot; a
/// host-only cookie (`example.com`) must be sent with an EMPTY domain (CEF then creates a
/// host cookie for the URL's host).
pub fn cef_cookie_domain(domain: &str) -> &str {
    if domain.starts_with('.') { domain } else { "" }
}

/// Does `cookie_domain` belong to `filter` (`example.com` matches `example.com`,
/// `.example.com` and `www.example.com`)? An empty filter matches everything.
pub fn domain_matches(cookie_domain: &str, filter: &str) -> bool {
    let filter = filter.trim().trim_start_matches('.').to_ascii_lowercase();
    if filter.is_empty() {
        return true;
    }
    let domain = cookie_domain.trim_start_matches('.').to_ascii_lowercase();
    domain == filter || domain.ends_with(&format!(".{filter}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending() -> PendingCookie {
        PendingCookie {
            id: 7,
            domain: ".example.com".into(),
            name: "sid".into(),
            value: "abc".into(),
            path: String::new(),
            expires: Some(1_800_000_000_000),
            secure: true,
            http_only: true,
            same_site: "None".into(),
        }
    }

    #[test]
    fn from_pending() {
        let c = CookieInfo::from(&pending());
        assert_eq!(c.path, "/");
        assert_eq!(c.same_site, "no_restriction");
        assert_eq!(cookie_url(&c), "https://example.com/");
    }

    #[test]
    fn time_round_trip() {
        let ms = 1_800_000_000_123;
        assert_eq!(cef_to_unix_ms(unix_ms_to_cef(ms)), Some(ms));
        assert_eq!(unix_ms_to_cef(0), WINDOWS_TO_UNIX_US);
        assert_eq!(cef_to_unix_ms(0), None);
    }

    #[test]
    fn same_site_spellings() {
        assert_eq!(normalize_same_site("lax"), "lax");
        assert_eq!(normalize_same_site("Strict"), "strict");
        assert_eq!(normalize_same_site("no-restriction"), "no_restriction");
        assert_eq!(normalize_same_site("whatever"), "unspecified");
    }

    #[test]
    fn host_only_cookies_have_no_domain_for_cef() {
        assert_eq!(cef_cookie_domain(".a.test"), ".a.test");
        assert_eq!(cef_cookie_domain("a.test"), "");
    }

    #[test]
    fn domain_filter() {
        assert!(domain_matches(".example.com", "example.com"));
        assert!(domain_matches("www.example.com", ".example.com"));
        assert!(!domain_matches("badexample.com", "example.com"));
        assert!(domain_matches("a.test", ""));
    }

    #[test]
    fn url_for_plain_cookie() {
        let c = CookieInfo {
            secure: false,
            domain: "a.test".into(),
            path: "/x".into(),
            ..CookieInfo::from(&pending())
        };
        assert_eq!(cookie_url(&c), "http://a.test/x");
    }
}
