//! Userscript headers and URL matching (a Tampermonkey / Chrome match-pattern subset).
//!
//! ```text
//! // ==UserScript==
//! // @name     Example banner
//! // @match    https://*.example.com/*
//! // @exclude  *://*/*.pdf
//! // @run-at   document-end
//! // @timeout  30
//! // ==/UserScript==
//! ```
//!
//! The header is the block of `//` comment lines at the top of the file (the `==UserScript==`
//! markers are optional). Keys:
//!
//! * `@match <pattern>` — Chrome match pattern `scheme://host/path`:
//!   * scheme `*` = `http` or `https`; otherwise `http`, `https`, `file` or `ftp`;
//!   * host `*` = any host, `*.example.com` = `example.com` and every subdomain, else exact
//!     (case-insensitive). A pattern host without a port matches any port; `host:8080` only
//!     that port;
//!   * path: `*` matches any run of characters; it is matched against path + query (not the
//!     `#fragment`);
//!   * `<all_urls>` = every `http`, `https`, `file` and `ftp` URL.
//! * `@include <glob>` / `@exclude <glob>` — `*` glob over the whole URL (no `/regex/` form);
//! * `@exclude-match <pattern>` — like `@match`, but excludes;
//! * `@run-at document-start | document-end | document-idle` (default `document-idle`; `end`
//!   and `idle` both run after the load finished, `start` as soon as the new URL is known);
//! * `@name`, `@description`, `@version` — shown in `userscripts.list`;
//! * `@timeout <seconds>` — limit for one run (BrowserMCP extension; default 60).
//!
//! A script runs when some `@match` or `@include` matches and no `@exclude` /
//! `@exclude-match` does.

use bmcp_api::access::glob_match;
use schemars::JsonSchema;
use serde::Serialize;

/// When a userscript runs during a navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
    DocumentIdle,
}

impl RunAt {
    /// Runs when the page is fully loaded (`document-end` and `document-idle`).
    pub fn after_load(self) -> bool {
        !matches!(self, RunAt::DocumentStart)
    }
}

/// The parsed header of a userscript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Header {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub matches: Vec<String>,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub exclude_matches: Vec<String>,
    pub run_at: RunAt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl Header {
    /// Read the header comment block at the top of `code`.
    pub fn parse(code: &str) -> Result<Header, String> {
        let mut header = Header {
            name: None,
            description: None,
            version: None,
            matches: Vec::new(),
            includes: Vec::new(),
            excludes: Vec::new(),
            exclude_matches: Vec::new(),
            run_at: RunAt::DocumentIdle,
            timeout_secs: None,
        };
        for line in code.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() {
                continue;
            }
            let Some(comment) = line.strip_prefix("//") else {
                break; // first line of code: the header is over
            };
            let comment = comment.trim();
            if comment.starts_with("==/UserScript==") {
                break;
            }
            let Some(rest) = comment.strip_prefix('@') else {
                continue; // ordinary comment, `==UserScript==`, `/// <reference …>`
            };
            let (key, value) = match rest.split_once(char::is_whitespace) {
                Some((key, value)) => (key, value.trim()),
                None => (rest, ""),
            };
            let value_owned = value.to_string();
            match key {
                "name" => header.name = Some(value_owned),
                "description" => header.description = Some(value_owned),
                "version" => header.version = Some(value_owned),
                "match" => {
                    MatchPattern::parse(value)?;
                    header.matches.push(value_owned);
                }
                "exclude-match" => {
                    MatchPattern::parse(value)?;
                    header.exclude_matches.push(value_owned);
                }
                "include" => header.includes.push(value_owned),
                "exclude" => header.excludes.push(value_owned),
                "run-at" => {
                    header.run_at = match value {
                        "document-start" => RunAt::DocumentStart,
                        "document-end" => RunAt::DocumentEnd,
                        "document-idle" => RunAt::DocumentIdle,
                        other => return Err(format!("unknown @run-at {other:?}")),
                    }
                }
                "timeout" => {
                    let secs: u64 = value
                        .parse()
                        .map_err(|_| format!("@timeout must be whole seconds, got {value:?}"))?;
                    header.timeout_secs = Some(secs.clamp(1, 3600));
                }
                _ => {} // @grant, @namespace, … are accepted and ignored
            }
        }
        if header.matches.is_empty() && header.includes.is_empty() {
            return Err(
                "no `// @match <pattern>` (or `// @include <glob>`) line in the header".into(),
            );
        }
        Ok(header)
    }

    /// Should the script run on `url`?
    pub fn applies_to(&self, url: &str) -> bool {
        let included = self
            .matches
            .iter()
            .any(|p| MatchPattern::parse(p).is_ok_and(|p| p.matches(url)))
            || self.includes.iter().any(|g| glob_match(g, url));
        let excluded = self.excludes.iter().any(|g| glob_match(g, url))
            || self
                .exclude_matches
                .iter()
                .any(|p| MatchPattern::parse(p).is_ok_and(|p| p.matches(url)));
        included && !excluded
    }
}

/// A parsed Chrome match pattern (see the [module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchPattern {
    AllUrls,
    Pattern {
        /// `None` = `*` (http or https).
        scheme: Option<String>,
        /// `None` = any host; `Some((host, subdomains))`.
        host: Option<(String, bool)>,
        /// `None` = any port.
        port: Option<String>,
        path: String,
    },
}

const SCHEMES: &[&str] = &["http", "https", "file", "ftp"];

impl MatchPattern {
    pub fn parse(pattern: &str) -> Result<MatchPattern, String> {
        let bad = |why: &str| Err(format!("bad @match {pattern:?}: {why}"));
        if pattern == "<all_urls>" {
            return Ok(MatchPattern::AllUrls);
        }
        let Some((scheme, rest)) = pattern.split_once("://") else {
            return bad("expected scheme://host/path");
        };
        let scheme = match scheme {
            "*" => None,
            s if SCHEMES.contains(&s) => Some(s.to_string()),
            _ => return bad("scheme must be *, http, https, file or ftp"),
        };
        let Some(slash) = rest.find('/') else {
            return bad("missing path (add /* at the end)");
        };
        let (authority, path) = rest.split_at(slash);
        let (host, port) = split_port(authority);
        let host = match host {
            "*" => None,
            "" if scheme.as_deref() == Some("file") => Some((String::new(), false)),
            "" => return bad("missing host"),
            h if h.contains('*') => match h.strip_prefix("*.") {
                Some(base) if !base.is_empty() && !base.contains('*') => {
                    Some((base.to_ascii_lowercase(), true))
                }
                _ => return bad("`*` in the host only as `*` or a leading `*.`"),
            },
            h => Some((h.to_ascii_lowercase(), false)),
        };
        Ok(MatchPattern::Pattern {
            scheme,
            host,
            port: port.filter(|p| *p != "*").map(str::to_string),
            path: path.to_string(),
        })
    }

    pub fn matches(&self, url: &str) -> bool {
        let Some(url) = ParsedUrl::parse(url) else {
            return false;
        };
        match self {
            MatchPattern::AllUrls => SCHEMES.contains(&url.scheme.as_str()),
            MatchPattern::Pattern {
                scheme,
                host,
                port,
                path,
            } => {
                let scheme_ok = match scheme {
                    None => url.scheme == "http" || url.scheme == "https",
                    Some(s) => *s == url.scheme,
                };
                let host_ok = match host {
                    None => true,
                    Some((h, false)) => *h == url.host,
                    Some((h, true)) => {
                        url.host == *h
                            || url
                                .host
                                .strip_suffix(h.as_str())
                                .is_some_and(|sub| sub.ends_with('.'))
                    }
                };
                let port_ok = port.as_ref().is_none_or(|p| Some(p) == url.port.as_ref());
                scheme_ok && host_ok && port_ok && glob_match(path, &url.path_and_query)
            }
        }
    }
}

/// `host:port` → (`host`, `Some(port)`); IPv6 `[::1]:80` keeps its brackets.
fn split_port(authority: &str) -> (&str, Option<&str>) {
    let colon = if authority.starts_with('[') {
        authority.find("]:").map(|i| i + 1)
    } else {
        authority.rfind(':')
    };
    match colon {
        Some(i) => (&authority[..i], Some(&authority[i + 1..])),
        None => (authority, None),
    }
}

/// Just enough URL parsing for matching.
struct ParsedUrl {
    scheme: String,
    host: String,
    port: Option<String>,
    path_and_query: String,
}

impl ParsedUrl {
    fn parse(url: &str) -> Option<ParsedUrl> {
        let (scheme, rest) = url.split_once("://")?;
        let rest = rest.split('#').next().unwrap_or_default();
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        // Drop `user:password@`.
        let authority = authority.rsplit('@').next().unwrap_or_default();
        let (host, port) = split_port(authority);
        let path_and_query = match tail {
            "" => "/".to_string(),
            t if t.starts_with('?') => format!("/{t}"),
            t => t.to_string(),
        };
        Some(ParsedUrl {
            scheme: scheme.to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port: port.map(str::to_string),
            path_and_query,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, url: &str) -> bool {
        MatchPattern::parse(pattern).unwrap().matches(url)
    }

    #[test]
    fn match_patterns() {
        // Subdomain wildcard covers the bare domain too.
        assert!(m("https://*.example.com/*", "https://example.com/"));
        assert!(m(
            "https://*.example.com/*",
            "https://www.example.com/a/b?q=1"
        ));
        assert!(m("https://*.example.com/*", "https://a.b.example.com/"));
        assert!(!m("https://*.example.com/*", "https://notexample.com/"));
        assert!(!m(
            "https://*.example.com/*",
            "https://example.com.evil.test/"
        ));
        assert!(!m("https://*.example.com/*", "http://example.com/"));
        // Scheme `*` = http or https only.
        assert!(m("*://example.com/*", "http://example.com/x"));
        assert!(m("*://example.com/*", "https://example.com/x"));
        assert!(!m("*://example.com/*", "ftp://example.com/x"));
        // Paths: glob over path + query, never the fragment.
        assert!(m("https://example.com/foo*", "https://example.com/foobar"));
        assert!(!m("https://example.com/foo*", "https://example.com/bar"));
        assert!(m(
            "https://example.com/*?x=1",
            "https://example.com/a?x=1#frag"
        ));
        assert!(m("https://example.com/", "https://example.com"));
        assert!(!m("https://example.com/", "https://example.com/a"));
        // Host is case-insensitive; ports: none in pattern = any.
        assert!(m("https://EXAMPLE.com/*", "https://example.COM:8443/"));
        assert!(m("http://localhost:8080/*", "http://localhost:8080/x"));
        assert!(!m("http://localhost:8080/*", "http://localhost:9090/x"));
        assert!(m("*://*/*", "https://anything.test/"));
        // Credentials in the URL don't fool the host check.
        assert!(!m(
            "https://example.com/*",
            "https://example.com@evil.test/"
        ));
        // <all_urls>
        assert!(m("<all_urls>", "file:///C:/x.html"));
        assert!(!m("<all_urls>", "bmcp://ui/newtab.html"));
        assert!(!m("*://*/*", "about:blank"));
    }

    #[test]
    fn bad_patterns() {
        for bad in [
            "example.com/*",
            "chrome://*/*",
            "https://*example.com/*",
            "https://ex*.com/*",
            "https://example.com",
            "https:///x",
        ] {
            assert!(MatchPattern::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn headers() {
        let code = "// ==UserScript==\n\
                    // @name        Banner\n\
                    // @match       https://example.com/*\n\
                    // @match       https://*.example.org/*\n\
                    // @exclude     *://*/private*\n\
                    // @run-at      document-start\n\
                    // @grant       none\n\
                    // ==/UserScript==\n\
                    // @match https://ignored.test/*\n\
                    console.log('hi');\n";
        let h = Header::parse(code).unwrap();
        assert_eq!(h.name.as_deref(), Some("Banner"));
        assert_eq!(
            h.matches.len(),
            2,
            "lines after ==/UserScript== are not header"
        );
        assert_eq!(h.run_at, RunAt::DocumentStart);
        assert!(h.applies_to("https://example.com/page"));
        assert!(h.applies_to("https://www.example.org/"));
        assert!(!h.applies_to("https://example.com/private/1"));
        assert!(!h.applies_to("https://ignored.test/"));

        // Without markers: the leading comment block, until the first code line.
        let h = Header::parse("/// <reference path=\"x\" />\n// @include *github.com*\nlet x;\n// @match https://late.test/*").unwrap();
        assert_eq!(h.run_at, RunAt::DocumentIdle);
        assert!(h.applies_to("https://github.com/x"));
        assert!(!h.applies_to("https://late.test/"));

        assert!(Header::parse("// @name nothing to match\n").is_err());
        assert!(Header::parse("// @match nope\n").is_err());
        assert!(Header::parse("// @match https://a.test/*\n// @run-at whenever\n").is_err());
        assert_eq!(
            Header::parse("// @match https://a.test/*\n// @timeout 5\n")
                .unwrap()
                .timeout_secs,
            Some(5)
        );
    }
}
