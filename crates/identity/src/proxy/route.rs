//! Split routing per profile: "these sites through that proxy, the rest direct" as a PAC
//! script (`FindProxyForURL`) and the Chromium `proxy` preference that loads it.
//!
//! Everything here is pure (no I/O), so the generated script and the Rust matcher used by
//! `proxy.rules_test` are unit-tested side by side.
//!
//! ## Proxy references (`via`, `proxy_id`)
//!
//! | text                   | means                                                         |
//! |------------------------|---------------------------------------------------------------|
//! | `direct`               | no proxy                                                      |
//! | `fwd-3`                | a running local forwarder (`proxy.forward_start`)             |
//! | `pool:NAME`            | a proxy of a pool (per profile it stays the same: a lease)    |
//! | `lease:POOL/RUN/ROW`   | the proxy `proxy.lease` gave this run row                     |
//! | a proxy URL            | `socks5://user:pass@host:port`, `http://host:port`, ...       |
//!
//! ## Match patterns (`match`)
//!
//! * `re:REGEX` — a regular expression searched in the URL (case-insensitive; the common
//!   subset of Rust and JavaScript syntax: no `(?flags)`, `\A`, `\z`, `\p{..}`);
//! * a pattern with `://` — a glob over the whole URL (`https://example.com/api/*`);
//! * anything else — a glob over the host name (`*.example.com`, `10.0.0.*`). A leading `*.`
//!   also matches the bare domain (`*.example.com` matches `example.com`).
//!
//! Globs use `*` (any characters) and `?` (one character) like PAC's `shExpMatch`; all
//! matching is case-insensitive. Note that Chromium passes only `scheme://host:port/` of
//! `https://` and `wss://` URLs to the PAC script (no path or query), so URL patterns with a
//! path only work for plain `http://`. [`route`] applies the same stripping.
//!
//! Proxies with a password cannot be written into a PAC script (Chromium would ask the user),
//! so every proxy is reached through a local forwarder; the PAC script then only contains
//! `SOCKS5 127.0.0.1:<port>` entries and `DIRECT`. There is deliberately no `; DIRECT`
//! fallback after a proxy: when the proxy is down the request fails instead of leaking the
//! real IP.

use std::fmt;

use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::config::ProxyConfig;

/// Where traffic goes (see the module docs for the text forms).
#[derive(Clone, PartialEq, Eq)]
pub enum ProxyRef {
    Direct,
    /// A running forwarder, e.g. "fwd-3".
    Forwarder(String),
    /// A pool name.
    Pool(String),
    /// The proxy leased to one row of a run.
    Lease {
        pool: String,
        run: String,
        row: u64,
    },
    /// An upstream proxy given by URL.
    Url(ProxyConfig),
}

/// Names of pools and runs: 1-64 chars of a-z, A-Z, 0-9, '-', '_' and '.'.
pub(crate) fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl ProxyRef {
    /// Parse the text form. Errors are readable sentences.
    pub fn parse(text: &str) -> Result<ProxyRef, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("direct") {
            return Ok(ProxyRef::Direct);
        }
        if let Some(n) = text.strip_prefix("fwd-")
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            return Ok(ProxyRef::Forwarder(text.to_string()));
        }
        if let Some(pool) = text.strip_prefix("pool:") {
            if !valid_name(pool) {
                return Err(format!("bad pool name in {text:?}"));
            }
            return Ok(ProxyRef::Pool(pool.to_string()));
        }
        if let Some(rest) = text.strip_prefix("lease:") {
            let parts: Vec<&str> = rest.split('/').collect();
            let [pool, run, row] = parts[..] else {
                return Err(format!("{text:?} is not lease:POOL/RUN/ROW"));
            };
            let row = row
                .parse::<u64>()
                .map_err(|_| format!("row in {text:?} is not a number"))?;
            if !valid_name(pool) || !valid_name(run) {
                return Err(format!("bad pool or run name in {text:?}"));
            }
            return Ok(ProxyRef::Lease {
                pool: pool.into(),
                run: run.into(),
                row,
            });
        }
        // A bare word ("de") would otherwise become the HTTP proxy "de:80".
        if !text.contains(':') {
            return Err(format!(
                "{text:?} is not a proxy reference: use direct, fwd-N, pool:NAME, \
                 lease:POOL/RUN/ROW or a proxy URL with a port"
            ));
        }
        text.parse::<ProxyConfig>()
            .map(ProxyRef::Url)
            .map_err(|e| e.to_string())
    }

    /// The text form with credentials (for storing).
    pub fn to_stored(&self) -> String {
        match self {
            ProxyRef::Url(p) => p.url(),
            other => other.to_string(),
        }
    }
}

/// Display masks passwords (`socks5://u:***@h:1`).
impl fmt::Display for ProxyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProxyRef::Direct => f.write_str("direct"),
            ProxyRef::Forwarder(id) => f.write_str(id),
            ProxyRef::Pool(p) => write!(f, "pool:{p}"),
            ProxyRef::Lease { pool, run, row } => write!(f, "lease:{pool}/{run}/{row}"),
            ProxyRef::Url(p) => f.write_str(&p.masked()),
        }
    }
}

impl fmt::Debug for ProxyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The lease key of a lease reference: "lease:pool/run/row".
pub fn lease_ref(pool: &str, run: &str, row: u64) -> String {
    format!("lease:{pool}/{run}/{row}")
}

/// What a rule matches (parsed from its `match` text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Glob over the host name (lower case).
    Host(String),
    /// Glob over the URL (lower case).
    Url(String),
    /// Regex searched in the URL (case-insensitive).
    Regex(String),
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Pattern, String> {
        let text = text.trim();
        if let Some(re) = text.strip_prefix("re:") {
            check_regex(re)?;
            return Ok(Pattern::Regex(re.to_string()));
        }
        if text.is_empty() {
            return Err("empty match pattern".into());
        }
        if text.chars().any(char::is_whitespace) {
            return Err(format!("match pattern {text:?} contains spaces"));
        }
        Ok(if text.contains("://") {
            Pattern::Url(text.to_ascii_lowercase())
        } else {
            Pattern::Host(text.to_ascii_lowercase())
        })
    }

    /// Does `url` (already stripped like Chromium does, see [`pac_url`]) with host `host`
    /// match? The same logic as the generated JavaScript.
    pub fn matches(&self, url: &str, host: &str) -> bool {
        let url = url.to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        match self {
            Pattern::Host(glob) => {
                glob_match(glob, &host) || glob.strip_prefix("*.").is_some_and(|bare| bare == host)
            }
            Pattern::Url(glob) => glob_match(glob, &url),
            Pattern::Regex(re) => regex::RegexBuilder::new(re)
                .case_insensitive(true)
                .build()
                .is_ok_and(|r| r.is_match(&url)),
        }
    }

    /// The JavaScript condition for the PAC script (`u` = lower-case URL, `h` = lower-case
    /// host). Strings are JSON-encoded, which is valid JavaScript, so nothing can break out.
    fn js_condition(&self) -> String {
        let s = |v: &str| Value::String(v.to_string()).to_string();
        match self {
            Pattern::Host(glob) => match glob.strip_prefix("*.") {
                Some(bare) => format!("(h === {} || shExpMatch(h, {}))", s(bare), s(glob)),
                None => format!("shExpMatch(h, {})", s(glob)),
            },
            Pattern::Url(glob) => format!("shExpMatch(u, {})", s(glob)),
            Pattern::Regex(re) => format!("new RegExp({}, \"i\").test(u)", s(re)),
        }
    }
}

/// Accept only regexes that mean the same in Rust (`rules_test`) and JavaScript (PAC).
fn check_regex(re: &str) -> Result<(), String> {
    if re.is_empty() {
        return Err("empty regex".into());
    }
    for bad in ["\\A", "\\z", "\\Z", "\\p{", "\\P{", "(?P<"] {
        if re.contains(bad) {
            return Err(format!(
                "regex {re:?}: {bad} is not supported (it differs between Rust and JavaScript)"
            ));
        }
    }
    // Groups like (?i) or (?<name>...) — only the plain non-capturing (?:...) is allowed.
    let mut rest = re;
    while let Some(i) = rest.find("(?") {
        if !rest[i..].starts_with("(?:") && !rest[..i].ends_with('\\') {
            return Err(format!(
                "regex {re:?}: only (?:...) groups are supported (matching is always \
                 case-insensitive)"
            ));
        }
        rest = &rest[i + 2..];
    }
    regex::Regex::new(re)
        .map(|_| ())
        .map_err(|e| format!("bad regex {re:?}: {e}"))
}

/// PAC-style glob: `*` = any characters, `?` = one character, the whole text must match.
pub fn glob_match(glob: &str, text: &str) -> bool {
    let g: Vec<char> = glob.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // Classic two-pointer wildcard match with backtracking to the last '*'.
    let (mut gi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if gi < g.len() && (g[gi] == '?' || g[gi] == t[ti]) {
            gi += 1;
            ti += 1;
        } else if gi < g.len() && g[gi] == '*' {
            star = Some((gi, ti));
            gi += 1;
        } else if let Some((sg, st)) = star {
            gi = sg + 1;
            ti = st + 1;
            star = Some((sg, st + 1));
        } else {
            return false;
        }
    }
    g[gi..].iter().all(|c| *c == '*')
}

/// One routing rule as stored in `proxies.json` (`routes.<profile>.rules`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Host glob (`*.example.com`), URL glob (`https://example.com/*`) or `re:REGEX`.
    #[serde(rename = "match")]
    pub pattern: String,
    /// `direct`, `fwd-N`, `pool:NAME`, `lease:POOL/RUN/ROW` or a proxy URL.
    pub via: String,
}

/// A profile's routing: rules checked in order, then the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Where everything else goes (default `direct`).
    #[serde(default = "direct")]
    pub default: String,
}

fn direct() -> String {
    "direct".into()
}

impl Default for RuleSet {
    fn default() -> Self {
        RuleSet {
            rules: Vec::new(),
            default: direct(),
        }
    }
}

/// A rule set with every pattern and reference parsed.
#[derive(Debug, Clone)]
pub struct Parsed {
    pub rules: Vec<(Pattern, ProxyRef)>,
    pub default: ProxyRef,
}

impl RuleSet {
    /// Parse and check every rule (errors name the rule).
    pub fn parse(&self) -> Result<Parsed, String> {
        let rules = self
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = Pattern::parse(&r.pattern).map_err(|e| format!("rule {}: {e}", i + 1))?;
                let v = ProxyRef::parse(&r.via).map_err(|e| format!("rule {}: {e}", i + 1))?;
                Ok((p, v))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let default = ProxyRef::parse(&self.default).map_err(|e| format!("default: {e}"))?;
        Ok(Parsed { rules, default })
    }

    /// The same rules with passwords masked (for output).
    pub fn masked(&self) -> RuleSet {
        let mask = |via: &str| {
            ProxyRef::parse(via)
                .map(|r| r.to_string())
                .unwrap_or_else(|_| via.to_string())
        };
        RuleSet {
            rules: self
                .rules
                .iter()
                .map(|r| Rule {
                    pattern: r.pattern.clone(),
                    via: mask(&r.via),
                })
                .collect(),
            default: mask(&self.default),
        }
    }

    /// Nothing to route: no rules and everything direct.
    pub fn is_direct(&self) -> bool {
        self.rules.is_empty() && ProxyRef::parse(&self.default) == Ok(ProxyRef::Direct)
    }
}

/// `scheme`, URL as the PAC script sees it, and the host, for a full URL. Chromium strips
/// the path, query and fragment of `https://`/`wss://` URLs (and always user:pass and the
/// fragment) before calling `FindProxyForURL`.
pub fn pac_url(url: &str) -> Result<(String, String), String> {
    let url = url.trim();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("{url:?} is not an absolute URL"))?;
    let scheme = scheme.to_ascii_lowercase();
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let path = rest[end..].split('#').next().unwrap_or("");
    // Drop user:pass@.
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split(']').next().unwrap_or("").to_string()
    } else {
        hostport.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        return Err(format!("no host in {url:?}"));
    }
    let path = if matches!(scheme.as_str(), "https" | "wss") || path.is_empty() {
        "/"
    } else {
        path
    };
    Ok((
        format!("{scheme}://{hostport}{path}").to_ascii_lowercase(),
        host.to_ascii_lowercase(),
    ))
}

/// Which way a URL goes ([`route`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Index of the matching rule (`None` = the default).
    pub rule: Option<usize>,
    pub via: ProxyRef,
    /// What the PAC script was given.
    pub pac_url: String,
    pub host: String,
}

/// Evaluate the rules for `url` exactly like the generated PAC script does.
pub fn route(parsed: &Parsed, url: &str) -> Result<Route, String> {
    let (pac_url, host) = pac_url(url)?;
    let hit = parsed
        .rules
        .iter()
        .position(|(p, _)| p.matches(&pac_url, &host));
    let via = match hit {
        Some(i) => parsed.rules[i].1.clone(),
        None => parsed.default.clone(),
    };
    Ok(Route {
        rule: hit,
        via,
        pac_url,
        host,
    })
}

/// A PAC result: `DIRECT` or `SOCKS5 127.0.0.1:<port>`.
pub fn pac_target(local_socks5: Option<std::net::SocketAddr>) -> String {
    match local_socks5 {
        Some(addr) => format!("SOCKS5 {addr}"),
        None => "DIRECT".into(),
    }
}

/// The PAC script for rules whose targets are already PAC results (see [`pac_target`]).
pub fn pac_script(rules: &[(Pattern, String)], default: &str) -> String {
    let s = |v: &str| Value::String(v.to_string()).to_string();
    let mut out = String::from(
        "// Generated by BrowserMCP (proxy.rules_set). Rules are checked in order.\n\
         function FindProxyForURL(url, host) {\n  \
         var u = String(url).toLowerCase();\n  \
         var h = String(host).toLowerCase();\n",
    );
    for (pattern, target) in rules {
        out.push_str(&format!(
            "  if ({}) return {};\n",
            pattern.js_condition(),
            s(target)
        ));
    }
    out.push_str(&format!("  return {};\n}}\n", s(default)));
    out
}

/// `data:` URL holding a PAC script (what Chromium's `pac_url` accepts inline).
pub fn pac_data_url(script: &str) -> String {
    format!(
        "data:application/x-ns-proxy-autoconfig;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(script)
    )
}

/// The value for `CefRequestContext::SetPreference("proxy", ..)` (Chromium's
/// `ProxyConfigDictionary`):
///
/// * no rules, default direct → `{"mode": "direct"}`;
/// * no rules, default proxy → `{"mode": "fixed_servers", "server": "socks5://127.0.0.1:p"}`;
/// * otherwise → `{"mode": "pac_script", "pac_url": "data:...", "pac_mandatory": true}`
///   (`pac_mandatory`: if the script fails, requests fail instead of going direct).
pub fn chromium_pref(rules: &[(Pattern, String)], default: &str) -> Value {
    if rules.is_empty() {
        return match default.strip_prefix("SOCKS5 ") {
            Some(addr) => json!({ "mode": "fixed_servers", "server": format!("socks5://{addr}") }),
            None => json!({ "mode": "direct" }),
        };
    }
    json!({
        "mode": "pac_script",
        "pac_url": pac_data_url(&pac_script(rules, default)),
        "pac_mandatory": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &[(&str, &str)], default: &str) -> Parsed {
        RuleSet {
            rules: list
                .iter()
                .map(|(m, v)| Rule {
                    pattern: m.to_string(),
                    via: v.to_string(),
                })
                .collect(),
            default: default.to_string(),
        }
        .parse()
        .unwrap()
    }

    #[test]
    fn parses_proxy_references() {
        assert_eq!(ProxyRef::parse("DIRECT"), Ok(ProxyRef::Direct));
        assert_eq!(
            ProxyRef::parse("fwd-12"),
            Ok(ProxyRef::Forwarder("fwd-12".into()))
        );
        assert_eq!(
            ProxyRef::parse("pool:res-de"),
            Ok(ProxyRef::Pool("res-de".into()))
        );
        assert_eq!(
            ProxyRef::parse("lease:de/run-7/42"),
            Ok(ProxyRef::Lease {
                pool: "de".into(),
                run: "run-7".into(),
                row: 42
            })
        );
        let url = ProxyRef::parse("socks5://u:secret@10.0.0.1:1080").unwrap();
        assert_eq!(url.to_string(), "socks5://u:***@10.0.0.1:1080");
        assert_eq!(url.to_stored(), "socks5://u:secret@10.0.0.1:1080");
        for bad in [
            "de",
            "pool:",
            "pool:a/b",
            "lease:de/run",
            "lease:de/run/x",
            "ftp://h:1",
        ] {
            assert!(ProxyRef::parse(bad).is_err(), "{bad} should fail");
        }
        assert_eq!(lease_ref("p", "r", 3), "lease:p/r/3");
    }

    #[test]
    fn globs_work_like_shexpmatch() {
        assert!(glob_match("*.example.com", "a.b.example.com"));
        assert!(!glob_match("*.example.com", "example.com"));
        assert!(glob_match("10.0.0.?", "10.0.0.7"));
        assert!(!glob_match("10.0.0.?", "10.0.0.17"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(glob_match("exact.com", "exact.com"));
        assert!(!glob_match("exact.com", "exact.com.evil"));
    }

    #[test]
    fn strips_urls_like_chromium() {
        assert_eq!(
            pac_url("https://User:pw@Example.COM:8443/path?q#f").unwrap(),
            (
                "https://example.com:8443/".to_string(),
                "example.com".to_string()
            )
        );
        assert_eq!(
            pac_url("http://example.com/a/B?x=1#frag").unwrap().0,
            "http://example.com/a/b?x=1"
        );
        assert_eq!(pac_url("http://[::1]:80/").unwrap().1, "::1");
        assert!(pac_url("example.com").is_err());
    }

    #[test]
    fn routes_in_order_with_default() {
        let parsed = rules(
            &[
                ("*.google.com", "direct"),
                ("re:\\.(ru|by)/", "pool:ru"),
                ("http://example.com/api/*", "fwd-1"),
                ("*", "socks5://u:p@h:1"),
            ],
            "direct",
        );
        let r = |url: &str| route(&parsed, url).unwrap();
        assert_eq!(
            r("https://google.com/").rule,
            Some(0),
            "bare domain matches *."
        );
        assert_eq!(r("https://mail.GOOGLE.com/x").via, ProxyRef::Direct);
        assert_eq!(r("https://yandex.ru/search?q=1").rule, Some(1));
        assert_eq!(r("http://example.com/api/v1").rule, Some(2));
        // https paths are invisible to PAC: the api rule cannot match.
        assert_eq!(r("https://example.com/api/v1").rule, Some(3));
        let none = rules(&[("*.a.com", "direct")], "fwd-2");
        let r = route(&none, "https://b.com/").unwrap();
        assert_eq!((r.rule, r.via), (None, ProxyRef::Forwarder("fwd-2".into())));
    }

    #[test]
    fn rejects_bad_rules() {
        for (m, v) in [
            ("re:(?i)x", "direct"),
            ("re:\\Ax", "direct"),
            ("re:(", "direct"),
            ("", "direct"),
            ("a b", "direct"),
            ("*.x.com", "nowhere"),
        ] {
            let set = RuleSet {
                rules: vec![Rule {
                    pattern: m.into(),
                    via: v.into(),
                }],
                default: "direct".into(),
            };
            let err = set.parse().unwrap_err();
            assert!(err.starts_with("rule 1:"), "{m}: {err}");
        }
        assert!(Pattern::parse("re:a(?:b|c)\\(?d").is_ok());
    }

    #[test]
    fn generates_the_pac_script_and_pref() {
        let local: std::net::SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let list = vec![
            (
                Pattern::parse("*.example.com").unwrap(),
                pac_target(Some(local)),
            ),
            (Pattern::parse("re:\"quote\"").unwrap(), pac_target(None)),
            (Pattern::parse("http://x.com/*").unwrap(), pac_target(None)),
        ];
        let script = pac_script(&list, "DIRECT");
        assert!(script.contains("function FindProxyForURL(url, host)"));
        assert!(script.contains(
            "if ((h === \"example.com\" || shExpMatch(h, \"*.example.com\"))) return \"SOCKS5 127.0.0.1:5000\";"
        ));
        // Quotes inside patterns are escaped, not able to end the string.
        assert!(
            script.contains("new RegExp(\"\\\"quote\\\"\", \"i\").test(u)"),
            "{script}"
        );
        assert!(script.contains("shExpMatch(u, \"http://x.com/*\")"));
        assert!(script.trim_end().ends_with("return \"DIRECT\";\n}"));
        assert!(
            !script.contains("; DIRECT"),
            "no direct fallback after a proxy"
        );

        let pref = chromium_pref(&list, "DIRECT");
        assert_eq!(pref["mode"], "pac_script");
        assert_eq!(pref["pac_mandatory"], true);
        let url = pref["pac_url"].as_str().unwrap();
        let b64 = url
            .strip_prefix("data:application/x-ns-proxy-autoconfig;base64,")
            .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), script);

        assert_eq!(chromium_pref(&[], "DIRECT"), json!({ "mode": "direct" }));
        assert_eq!(
            chromium_pref(&[], "SOCKS5 127.0.0.1:5000"),
            json!({ "mode": "fixed_servers", "server": "socks5://127.0.0.1:5000" })
        );
    }

    #[test]
    fn masks_passwords_in_rule_output() {
        let set = RuleSet {
            rules: vec![Rule {
                pattern: "*".into(),
                via: "http://u:secret@h:1".into(),
            }],
            default: "socks5://a:b2@h:2".into(),
        };
        let text = format!("{:?}", set.masked());
        assert!(!text.contains("secret") && !text.contains("b2@"), "{text}");
        assert!(RuleSet::default().is_direct());
    }
}
