//! [`ProxyConfig`]: one upstream proxy parsed from a URL.
//!
//! Accepted forms:
//!
//! * `http://user:pass@host:port`, `socks5://user:pass@host:port` (`socks5h://` = same thing:
//!   we always let the proxy resolve host names, so DNS does not leak);
//! * `host:port` and `user:pass@host:port` (HTTP);
//! * `host:port:user:pass` (the usual format of bought proxy lists, HTTP);
//! * IPv6 in brackets: `socks5://[2001:db8::1]:1080`.
//!
//! User and password may be percent-encoded (`p%40ss` = `p@ss`).

use std::fmt;
use std::str::FromStr;

/// Proxy protocol of the upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    /// HTTP proxy: `CONNECT host:port` tunnels (and absolute-form requests).
    Http,
    /// SOCKS5 with optional username/password (RFC 1928 / RFC 1929).
    Socks5,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Socks5 => "socks5",
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Socks5 => 1080,
        }
    }
}

/// One upstream proxy.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyConfig {
    pub scheme: Scheme,
    /// Host name or IP (IPv6 without brackets).
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Why a proxy URL was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

impl FromStr for ProxyConfig {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let bad = |why: &str| ParseError(format!("bad proxy {:?}: {why}", mask_input(input)));
        let text = input.trim().trim_end_matches('/');
        if text.is_empty() {
            return Err(bad("empty"));
        }
        let (scheme, rest) = match text.split_once("://") {
            Some((s, rest)) => {
                let scheme = match s.to_ascii_lowercase().as_str() {
                    "http" => Scheme::Http,
                    "socks5" | "socks5h" => Scheme::Socks5,
                    "https" => {
                        return Err(bad(
                            "https:// proxies (TLS to the proxy) are not supported; use http:// \
                             or socks5://",
                        ));
                    }
                    "socks4" | "socks4a" => {
                        return Err(bad("SOCKS4 is not supported; use socks5://"));
                    }
                    other => return Err(bad(&format!("unknown scheme {other:?}"))),
                };
                (scheme, rest)
            }
            None => (Scheme::Http, text),
        };
        if rest.contains('/') || rest.contains('?') {
            return Err(bad("a proxy URL has no path"));
        }

        // user:pass@host:port, or the list format host:port:user:pass.
        let (userinfo, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => (Some(u), h.to_string()),
            None => {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 4 && !rest.starts_with('[') {
                    let creds = format!("{}:{}", parts[2], parts[3]);
                    return finish(scheme, &format!("{}:{}", parts[0], parts[1]), Some(&creds))
                        .map_err(|why| bad(&why));
                }
                (None, rest.to_string())
            }
        };
        finish(scheme, &hostport, userinfo).map_err(|why| bad(&why))
    }
}

fn finish(scheme: Scheme, hostport: &str, userinfo: Option<&str>) -> Result<ProxyConfig, String> {
    let (host, port) = split_host_port(hostport)?;
    let port = match port {
        Some(p) => p
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("port {p:?} is not 1-65535"))?,
        None => scheme.default_port(),
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err("missing or invalid host".into());
    }
    let (username, password) = match userinfo {
        None => (None, None),
        Some(info) => {
            let (u, p) = info.split_once(':').unwrap_or((info, ""));
            let u = percent_decode(u)?;
            let p = percent_decode(p)?;
            if u.is_empty() {
                return Err("empty user name".into());
            }
            if u.len() > 255 || p.len() > 255 {
                return Err("user name and password must be at most 255 bytes".into());
            }
            (Some(u), Some(p))
        }
    };
    Ok(ProxyConfig {
        scheme,
        host,
        port,
        username,
        password,
    })
}

/// `host:port`, `[v6]:port`, `host` → (host, port?).
fn split_host_port(text: &str) -> Result<(String, Option<&str>), String> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or("unclosed '[' in IPv6 address")?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':').ok_or("expected ':' after ']'")?),
        };
        return Ok((host.to_string(), port));
    }
    match text.rsplit_once(':') {
        Some((host, _)) if host.contains(':') => {
            Err("IPv6 addresses must be written in brackets: [::1]:1080".into())
        }
        Some((host, port)) => Ok((host.to_string(), Some(port))),
        None => Ok((text.to_string(), None)),
    }
}

fn percent_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or("bad %-escape in user or password")?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "user or password is not UTF-8".into())
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Hide a password in user input that failed to parse (best effort).
fn mask_input(input: &str) -> String {
    match input.rsplit_once('@') {
        Some((before, after)) => match before.rsplit_once(':') {
            Some((user, _)) if !user.ends_with('/') => format!("{user}:***@{after}"),
            _ => input.to_string(),
        },
        None => input.to_string(),
    }
}

impl ProxyConfig {
    fn host_port(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Full URL including credentials (for storing).
    pub fn url(&self) -> String {
        match &self.username {
            Some(u) => format!(
                "{}://{}:{}@{}",
                self.scheme.as_str(),
                percent_encode(u),
                percent_encode(self.password.as_deref().unwrap_or("")),
                self.host_port()
            ),
            None => format!("{}://{}", self.scheme.as_str(), self.host_port()),
        }
    }

    /// URL with the password replaced by `***` (for output and logs).
    pub fn masked(&self) -> String {
        match &self.username {
            Some(u) => format!(
                "{}://{}:***@{}",
                self.scheme.as_str(),
                percent_encode(u),
                self.host_port()
            ),
            None => self.url(),
        }
    }

    pub fn has_auth(&self) -> bool {
        self.username.is_some()
    }
}

/// Debug never prints the password.
impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.masked())
    }
}

impl fmt::Display for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.masked())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ProxyConfig {
        s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn parses_common_forms() {
        let p = parse("http://user:pass@1.2.3.4:8080");
        assert_eq!(p.scheme, Scheme::Http);
        assert_eq!((p.host.as_str(), p.port), ("1.2.3.4", 8080));
        assert_eq!(p.username.as_deref(), Some("user"));
        assert_eq!(p.password.as_deref(), Some("pass"));

        let p = parse("socks5://alice:s3cr%40t@proxy.example.com:1080");
        assert_eq!(p.scheme, Scheme::Socks5);
        assert_eq!(p.password.as_deref(), Some("s3cr@t"));
        assert_eq!(p.url(), "socks5://alice:s3cr%40t@proxy.example.com:1080");
        assert_eq!(p.masked(), "socks5://alice:***@proxy.example.com:1080");
        assert_eq!(format!("{p:?}"), p.masked());

        assert_eq!(parse("socks5h://h:1").scheme, Scheme::Socks5);
        let p = parse("proxy.local:3128");
        assert_eq!(
            (p.scheme, p.port, p.username.clone()),
            (Scheme::Http, 3128, None)
        );
        let p = parse("10.0.0.1:8000:bob:hunter2");
        assert_eq!(p.host, "10.0.0.1");
        assert_eq!(p.username.as_deref(), Some("bob"));
        assert_eq!(p.password.as_deref(), Some("hunter2"));
        let p = parse("bob:pw@10.0.0.1:8000");
        assert_eq!(p.username.as_deref(), Some("bob"));
        // Passwords may contain ':' and '@' when written in URL form.
        let p = parse("http://bob:a:b@c@h:1");
        assert_eq!(p.password.as_deref(), Some("a:b@c"));
        assert_eq!(parse("socks5://[::1]:9050").host, "::1");
        assert_eq!(parse("socks5://[::1]:9050").url(), "socks5://[::1]:9050");
        assert_eq!(parse("socks5://h").port, 1080);
        assert_eq!(parse("http://h/").port, 80);
        // Round trip through url().
        let p = parse("socks5://us er:p w@h:5");
        assert_eq!(parse(&p.url()), p);
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "https://h:1",
            "socks4://h:1",
            "ftp://h:1",
            "http://h:0",
            "http://h:99999",
            "http://h:1/path",
            "http://:1",
            "::1:80",
            "http://[::1:80",
            "http://:pw@h:1",
            "http://u:%zz@h:1",
        ] {
            assert!(bad.parse::<ProxyConfig>().is_err(), "{bad:?} should fail");
        }
        let err = "ftp://bob:secret@h:1".parse::<ProxyConfig>().unwrap_err();
        assert!(!err.0.contains("secret"), "{err}");
    }
}
