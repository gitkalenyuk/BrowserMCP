//! Per-site simplification policies (`data/config/simplify.json`) and the adblock settings
//! (`data/config/adblock.json`).
//!
//! A policy says, per host pattern, which kinds of content a site may load:
//!
//! ```json
//! { "sites": {
//!     "*":           { "third_party": false },
//!     "example.com": { "images": false, "fonts": false },
//!     "news.example.com": { "third_party": true } } }
//! ```
//!
//! `true` = allowed, `false` = blocked, missing = inherit. Patterns: `*` (every site),
//! `example.com` (the host and all its subdomains; `*.example.com` means the same). For a page
//! on `news.example.com` all three entries above match and are overlaid from the least to the
//! most specific one, so there third-party loads are allowed but images and fonts are not.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One site's rules; every field is optional (`None` = inherit from a less specific pattern).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SitePolicy {
    /// Run JavaScript (false: block script loads and disable script execution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js: Option<bool>,
    /// Load images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<bool>,
    /// Load web fonts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fonts: Option<bool>,
    /// Load audio/video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<bool>,
    /// Load anything from other sites (different registrable domain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub third_party: Option<bool>,
    /// Play CSS animations/transitions (false: disable them, prefer reduced motion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<bool>,
}

impl SitePolicy {
    /// `self` with every field that `top` sets replaced by `top`'s value.
    pub fn overlay(self, top: SitePolicy) -> SitePolicy {
        SitePolicy {
            js: top.js.or(self.js),
            images: top.images.or(self.images),
            fonts: top.fonts.or(self.fonts),
            media: top.media.or(self.media),
            third_party: top.third_party.or(self.third_party),
            animations: top.animations.or(self.animations),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == SitePolicy::default()
    }

    /// Fill in the defaults (everything allowed).
    pub fn resolve(&self) -> ResolvedPolicy {
        ResolvedPolicy {
            js: self.js.unwrap_or(true),
            images: self.images.unwrap_or(true),
            fonts: self.fonts.unwrap_or(true),
            media: self.media.unwrap_or(true),
            third_party: self.third_party.unwrap_or(true),
            animations: self.animations.unwrap_or(true),
        }
    }
}

/// A policy with every field decided (true = allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ResolvedPolicy {
    pub js: bool,
    pub images: bool,
    pub fonts: bool,
    pub media: bool,
    pub third_party: bool,
    pub animations: bool,
}

impl Default for ResolvedPolicy {
    fn default() -> Self {
        SitePolicy::default().resolve()
    }
}

impl ResolvedPolicy {
    /// The CEF content settings this policy maps to (applied by the CEF crate later through
    /// `CefRequestContext::SetContentSetting`; the interim CDP path uses
    /// `Emulation.setScriptExecutionDisabled` for JavaScript instead).
    pub fn content_settings(&self) -> ContentSettings {
        let word = |allowed: bool| if allowed { "allow" } else { "block" };
        ContentSettings {
            javascript: word(self.js).into(),
            images: word(self.images).into(),
        }
    }

    /// True when nothing is restricted.
    pub fn is_default(&self) -> bool {
        *self == ResolvedPolicy::default()
    }
}

/// Content-setting values per CEF content type (`allow` or `block`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ContentSettings {
    /// CEF_CONTENT_SETTING_TYPE_JAVASCRIPT.
    pub javascript: String,
    /// CEF_CONTENT_SETTING_TYPE_IMAGES.
    pub images: String,
}

/// All site policies, keyed by normalized pattern. The file format of `simplify.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimplifyPolicies {
    #[serde(default)]
    pub sites: BTreeMap<String, SitePolicy>,
}

impl SimplifyPolicies {
    /// The merged policy for a page host, and the patterns that matched (least specific first).
    pub fn effective(&self, host: &str) -> (SitePolicy, Vec<String>) {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let mut matched: Vec<&String> = self
            .sites
            .keys()
            .filter(|pattern| host_matches(pattern, &host))
            .collect();
        matched.sort_by_key(|pattern| specificity(pattern));
        let policy = matched
            .iter()
            .fold(SitePolicy::default(), |acc, p| acc.overlay(self.sites[*p]));
        (policy, matched.into_iter().cloned().collect())
    }

    /// Merge `update` into the stored policy of `pattern` (fields it sets win); `None` fields
    /// keep the stored value. An entry left with no fields is removed.
    pub fn merge(&mut self, pattern: &str, update: SitePolicy) -> SitePolicy {
        let stored = self.sites.get(pattern).copied().unwrap_or_default();
        let merged = stored.overlay(update);
        if merged.is_empty() {
            self.sites.remove(pattern);
        } else {
            self.sites.insert(pattern.to_string(), merged);
        }
        merged
    }
}

/// Adblock on/off and the allowlist; the file format of `adblock.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdblockSettings {
    /// Global switch for filter-list blocking (site policies apply regardless).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Host patterns where filter lists are not applied (same syntax as site policies).
    #[serde(default)]
    pub allowlist: BTreeSet<String>,
    /// Ids of the filter lists to use (see `lists::CATALOG`).
    #[serde(default = "default_lists")]
    pub lists: Vec<String>,
}

impl Default for AdblockSettings {
    fn default() -> Self {
        AdblockSettings {
            enabled: true,
            allowlist: BTreeSet::new(),
            lists: default_lists(),
        }
    }
}

impl AdblockSettings {
    /// Is filtering switched off for pages on `host`?
    pub fn is_allowlisted(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.allowlist.iter().any(|p| host_matches(p, &host))
    }
}

fn yes() -> bool {
    true
}

fn default_lists() -> Vec<String> {
    crate::lists::DEFAULT_LISTS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Normalize a user-given pattern: `https://WWW.Example.com:443/x` → `www.example.com`,
/// `*.example.com` / `.example.com` → `example.com`, `*` stays `*`.
pub fn normalize_pattern(input: &str) -> Result<String, String> {
    let mut p = input.trim().to_ascii_lowercase();
    if p == "*" {
        return Ok(p);
    }
    if let Some(rest) = p.split_once("://").map(|(_, rest)| rest.to_string()) {
        p = rest;
    }
    // Drop path, query, port and user info.
    p = p
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_string();
    if let Some((_, host)) = p.rsplit_once('@') {
        p = host.to_string();
    }
    if let Some((host, port)) = p.rsplit_once(':')
        && port.chars().all(|c| c.is_ascii_digit())
    {
        p = host.to_string();
    }
    let p = p
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .trim_end_matches('.');
    let valid = !p.is_empty()
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
        && !p.contains("..");
    if valid {
        Ok(p.to_string())
    } else {
        Err(format!(
            "invalid host pattern {input:?}: use a host like example.com, or * for every site"
        ))
    }
}

/// Does a normalized pattern match `host` (lowercase)? `example.com` matches `example.com` and
/// `a.b.example.com`, not `badexample.com`.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    pattern == "*"
        || host == pattern
        || (host.len() > pattern.len()
            && host.ends_with(pattern)
            && host.as_bytes()[host.len() - pattern.len() - 1] == b'.')
}

/// `*` < `example.com` < `www.example.com`.
fn specificity(pattern: &str) -> usize {
    if pattern == "*" {
        0
    } else {
        1 + pattern.matches('.').count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(json: serde_json::Value) -> SitePolicy {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn patterns_are_normalized() {
        assert_eq!(
            normalize_pattern("https://WWW.Example.com:443/a?b").unwrap(),
            "www.example.com"
        );
        assert_eq!(normalize_pattern("*.example.com").unwrap(), "example.com");
        assert_eq!(normalize_pattern(" * ").unwrap(), "*");
        assert!(normalize_pattern("").is_err());
        assert!(normalize_pattern("a b").is_err());
    }

    #[test]
    fn host_matching_respects_label_boundaries() {
        assert!(host_matches("example.com", "example.com"));
        assert!(host_matches("example.com", "a.b.example.com"));
        assert!(!host_matches("example.com", "badexample.com"));
        assert!(host_matches("*", "anything.test"));
    }

    #[test]
    fn effective_policy_overlays_from_general_to_specific() {
        let mut all = SimplifyPolicies::default();
        all.merge("*", policy(serde_json::json!({"third_party": false})));
        all.merge(
            "example.com",
            policy(serde_json::json!({"images": false, "fonts": false})),
        );
        all.merge(
            "news.example.com",
            policy(serde_json::json!({"third_party": true, "fonts": true})),
        );
        let (p, matched) = all.effective("news.example.com");
        assert_eq!(matched, ["*", "example.com", "news.example.com"]);
        let r = p.resolve();
        assert!(r.third_party && r.fonts && !r.images && r.js);

        let (p, matched) = all.effective("shop.example.com");
        assert_eq!(matched, ["*", "example.com"]);
        assert!(!p.resolve().third_party && !p.resolve().fonts);

        let (p, _) = all.effective("other.test");
        assert_eq!(p.resolve().content_settings().javascript, "allow");
        assert!(!p.resolve().third_party);
    }

    #[test]
    fn merge_keeps_unset_fields_and_drops_empty_entries() {
        let mut all = SimplifyPolicies::default();
        all.merge("a.test", policy(serde_json::json!({"js": false})));
        let merged = all.merge("a.test", policy(serde_json::json!({"images": false})));
        assert_eq!(merged.js, Some(false));
        assert_eq!(merged.images, Some(false));
        assert_eq!(
            merged.resolve().content_settings(),
            ContentSettings {
                javascript: "block".into(),
                images: "block".into()
            }
        );
        // Unknown fields are rejected (typos must not silently do nothing).
        assert!(serde_json::from_value::<SitePolicy>(serde_json::json!({"img": false})).is_err());
    }

    #[test]
    fn allowlist_matches_subdomains() {
        let mut s = AdblockSettings::default();
        s.allowlist.insert("example.com".into());
        assert!(s.is_allowlisted("www.example.com"));
        assert!(!s.is_allowlisted("example.org"));
        // Old files without the newer fields still load, with defaults.
        let old: AdblockSettings = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert!(!old.enabled);
        assert_eq!(old.lists, ["easylist", "easyprivacy"]);
    }
}
