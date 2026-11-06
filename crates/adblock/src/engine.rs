//! [`RequestFilter`]: a thin wrapper around Brave's `adblock::Engine`.
//!
//! The engine is built once from filter-list text (EasyList syntax), then serialized to bytes
//! (`data/filters/engine.dat`) so the next start only has to deserialize it (milliseconds
//! instead of the ~1 s parse). With the crate's default `single-thread` feature the engine is
//! `!Send`/`!Sync`, so a `RequestFilter` never leaves the thread that built it: the
//! [`crate::gate::Gate`] keeps one per thread in a `thread_local!`, built from the shared bytes.

use std::collections::HashSet;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What kind of resource a request loads. A CEF-agnostic superset of what CEF
/// (`cef_resource_type_t`), CDP (`Network.ResourceType`) and the filter lists distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    /// Top-level page (a navigation).
    MainFrame,
    /// Frame or iframe document.
    SubFrame,
    Stylesheet,
    Script,
    Image,
    Font,
    /// `<object>`/`<embed>` and plugin requests.
    Object,
    /// Audio/video.
    Media,
    /// `XMLHttpRequest`.
    Xhr,
    /// `fetch()`.
    Fetch,
    /// `<a ping>`, `navigator.sendBeacon`.
    Ping,
    CspReport,
    WebSocket,
    /// Main script of a dedicated/shared/service worker.
    Worker,
    Favicon,
    Prefetch,
    Other,
}

impl ResourceType {
    /// The request type name the `adblock` crate understands (`$script`, `$image`, … options).
    pub fn adblock_name(self) -> &'static str {
        match self {
            ResourceType::MainFrame => "document",
            ResourceType::SubFrame => "sub_frame",
            ResourceType::Stylesheet => "stylesheet",
            // Filter lists have no worker type; uBlock/Brave treat worker scripts as scripts.
            ResourceType::Script | ResourceType::Worker => "script",
            ResourceType::Image | ResourceType::Favicon => "image",
            ResourceType::Font => "font",
            ResourceType::Object => "object",
            ResourceType::Media => "media",
            ResourceType::Xhr | ResourceType::Fetch => "xmlhttprequest",
            ResourceType::Ping => "ping",
            ResourceType::CspReport => "csp_report",
            ResourceType::WebSocket => "websocket",
            ResourceType::Prefetch | ResourceType::Other => "other",
        }
    }

    /// CDP `Network.ResourceType` (as sent in `Fetch.requestPaused.resourceType`).
    /// `is_main_frame` tells a top-level `Document` from an iframe one.
    pub fn from_cdp(name: &str, is_main_frame: bool) -> Self {
        match name {
            "Document" if is_main_frame => ResourceType::MainFrame,
            "Document" => ResourceType::SubFrame,
            "Stylesheet" => ResourceType::Stylesheet,
            "Script" => ResourceType::Script,
            "Image" => ResourceType::Image,
            "Font" => ResourceType::Font,
            "Media" | "TextTrack" => ResourceType::Media,
            "XHR" => ResourceType::Xhr,
            "Fetch" | "EventSource" => ResourceType::Fetch,
            "Ping" => ResourceType::Ping,
            "CSPViolationReport" => ResourceType::CspReport,
            "WebSocket" => ResourceType::WebSocket,
            "Prefetch" => ResourceType::Prefetch,
            _ => ResourceType::Other,
        }
    }

    /// CEF `cef_resource_type_t` value (`CefRequest::GetResourceType`). The numbers match
    /// Chromium's ResourceType and are never renumbered (see crates/adblock/README.md).
    pub fn from_cef(value: i32) -> Self {
        match value {
            0 | 19 => ResourceType::MainFrame, // RT_MAIN_FRAME, RT_NAVIGATION_PRELOAD_MAIN_FRAME
            1 | 20 => ResourceType::SubFrame,  // RT_SUB_FRAME, RT_NAVIGATION_PRELOAD_SUB_FRAME
            2 => ResourceType::Stylesheet,
            3 => ResourceType::Script,
            4 => ResourceType::Image,
            5 => ResourceType::Font,
            7 | 17 => ResourceType::Object, // RT_OBJECT, RT_PLUGIN_RESOURCE
            8 => ResourceType::Media,
            9 | 10 | 15 => ResourceType::Worker, // dedicated, shared, service worker
            11 => ResourceType::Prefetch,
            12 => ResourceType::Favicon,
            13 => ResourceType::Xhr, // CEF reports fetch() as RT_XHR too
            14 => ResourceType::Ping,
            16 => ResourceType::CspReport,
            _ => ResourceType::Other, // 6 = RT_SUB_RESOURCE and anything new
        }
    }

    /// Parse a user-given type: our snake_case names, filter-list names (`xmlhttprequest`,
    /// `subdocument`, `document`) or CDP names (`XHR`, `Document`).
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        let ty = match lower.as_str() {
            "main_frame" | "document" => ResourceType::MainFrame,
            "sub_frame" | "subdocument" | "iframe" => ResourceType::SubFrame,
            "stylesheet" | "css" => ResourceType::Stylesheet,
            "script" | "js" => ResourceType::Script,
            "image" | "img" => ResourceType::Image,
            "font" => ResourceType::Font,
            "object" => ResourceType::Object,
            "media" => ResourceType::Media,
            "xhr" | "xmlhttprequest" => ResourceType::Xhr,
            "fetch" => ResourceType::Fetch,
            "ping" | "beacon" => ResourceType::Ping,
            "csp_report" => ResourceType::CspReport,
            "websocket" => ResourceType::WebSocket,
            "worker" => ResourceType::Worker,
            "favicon" => ResourceType::Favicon,
            "prefetch" => ResourceType::Prefetch,
            "other" => ResourceType::Other,
            _ => return None,
        };
        Some(ty)
    }
}

/// The filter lists' verdict for one request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Decision {
    /// True when the request should be cancelled.
    pub block: bool,
    /// Replacement body as a `data:` URL (`$redirect=` rules). Only produced when redirect
    /// resources are loaded, which BrowserMCP does not do yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    /// An `$important` rule matched: exceptions do not apply.
    pub important: bool,
    /// An exception (`@@…`) rule matched.
    pub exception: bool,
    /// Text of the matched rule, when the engine kept it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// `$removeparam` result: load this URL instead (only when not blocked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewritten_url: Option<String>,
}

/// What a page needs for cosmetic (element hiding) filtering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CosmeticResources {
    /// CSS selectors to hide on this page (`example.com##.ad` plus generic complex selectors).
    pub hide_selectors: Vec<String>,
    /// Ready-to-inject CSS for `hide_selectors` (one rule per selector, so one bad selector
    /// cannot disable the others).
    pub style: String,
    /// JavaScript of the scriptlets (`##+js(...)`) to run in the page's main world. Empty
    /// unless scriptlet resources are loaded (not yet).
    pub scriptlets: String,
    /// A `$generichide` exception matched: skip the generic class/id pass.
    pub generichide: bool,
    /// Class/id selectors excluded from generic hiding; pass them to
    /// [`RequestFilter::generic_selectors`].
    #[serde(skip)]
    pub exceptions: HashSet<String>,
}

/// One compiled filter engine. Not `Send`: keep it on the thread that built it.
pub struct RequestFilter {
    engine: Engine,
}

impl RequestFilter {
    /// Build from filter-list texts (EasyList syntax). Unparsable lines are skipped.
    pub fn from_lists<S: AsRef<str>>(lists: &[S]) -> Self {
        // `debug = false`: do not keep the raw rule text (saves a lot of memory).
        let mut set = FilterSet::new(false);
        for list in lists {
            set.add_filter_list(list.as_ref().to_string(), ParseOptions::default());
        }
        RequestFilter {
            engine: Engine::new_with_filter_set(set),
        }
    }

    /// Load an engine produced by [`RequestFilter::serialize`]. Fails on corrupt data or data
    /// from an incompatible `adblock` version.
    pub fn from_serialized(bytes: &[u8]) -> Result<Self, String> {
        let mut engine = Engine::default();
        engine
            .deserialize(bytes)
            .map_err(|e| format!("cannot load the cached filter engine: {e:?}"))?;
        Ok(RequestFilter { engine })
    }

    /// The compact binary form, for `engine.dat` and for other threads.
    pub fn serialize(&self) -> Vec<u8> {
        self.engine.serialize()
    }

    /// Compile lists straight to the serialized form.
    pub fn compile<S: AsRef<str>>(lists: &[S]) -> Vec<u8> {
        Self::from_lists(lists).serialize()
    }

    /// Should a request for `url` of type `resource_type`, made by the page `source_url`
    /// (empty = unknown, treated as the request itself), be blocked?
    pub fn check(&self, url: &str, source_url: &str, resource_type: ResourceType) -> Decision {
        // data:, blob:, chrome:, bmcp: … are never in filter lists.
        if !is_http(url) {
            return Decision::default();
        }
        let source = if source_url.is_empty() {
            url
        } else {
            source_url
        };
        let Ok(request) = Request::new(url, source, resource_type.adblock_name(), "get") else {
            return Decision::default();
        };
        let result = self.engine.check_network_request(&request);
        let block = result.should_block();
        Decision {
            block,
            redirect: if block { result.redirect } else { None },
            important: result.important,
            exception: result.exception.is_some(),
            rule: result.filter.and_then(|f| f.raw_line).filter(|_| block),
            rewritten_url: if block { None } else { result.rewritten_url },
        }
    }

    /// Element-hiding rules and scriptlets for the page at `url`.
    pub fn cosmetic(&self, url: &str) -> CosmeticResources {
        if !is_http(url) {
            return CosmeticResources::default();
        }
        let resources = self.engine.url_cosmetic_resources(url);
        let mut hide_selectors: Vec<String> = resources.hide_selectors.into_iter().collect();
        hide_selectors.sort();
        CosmeticResources {
            style: hide_css(&hide_selectors),
            hide_selectors,
            scriptlets: resources.injected_script,
            generichide: resources.generichide,
            exceptions: resources.exceptions,
        }
    }

    /// Generic hiding rules (`##.ad-banner`, `###sponsor`) that apply to the classes and ids
    /// actually present on a page. `exceptions` comes from [`CosmeticResources::exceptions`].
    pub fn generic_selectors(
        &self,
        classes: &[String],
        ids: &[String],
        exceptions: &HashSet<String>,
    ) -> Vec<String> {
        let mut selectors = self
            .engine
            .hidden_class_id_selectors(classes, ids, exceptions);
        selectors.sort();
        selectors
    }
}

/// `sel { display: none !important; }` for every selector.
pub fn hide_css(selectors: &[String]) -> String {
    let mut css = String::new();
    for selector in selectors {
        css.push_str(selector);
        css.push_str("{display:none!important}\n");
    }
    css
}

/// Only http(s) and ws(s) requests are filtered.
pub fn is_http(url: &str) -> bool {
    let lower = url.get(..6).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("ws:")
        || lower.starts_with("wss:")
}

/// Hostname and registrable domain (eTLD+1, via the public suffix list) of a URL:
/// `https://cdn.ads.example.co.uk/x` → `("cdn.ads.example.co.uk", "example.co.uk")`.
pub fn host_and_domain(url: &str) -> Option<(String, String)> {
    let parsed = adblock::url_parser::parse_url(url)?;
    let host = parsed.hostname().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((host, parsed.domain().to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny list exercising every kind of rule we rely on.
    const RULES: &str = "\
[Adblock Plus 2.0]
! comment lines are ignored
||ads.example^
||tracker.test^$third-party
/banner/*$image
||cdn.test/lib.js$script
@@||ads.example/allowed/*
||evil.test^$important
@@||evil.test/ok^
news.test##.sponsored
##.ad-banner
###top-ad
news.test#@#.ad-banner
";

    fn filter() -> RequestFilter {
        RequestFilter::from_lists(&[RULES])
    }

    #[test]
    fn blocks_plain_rules_and_reports_them() {
        let f = filter();
        let d = f.check(
            "https://ads.example/pixel.gif",
            "https://news.test/",
            ResourceType::Image,
        );
        assert!(d.block, "{d:?}");
        assert!(!d.exception);
        let ok = f.check(
            "https://news.test/app.js",
            "https://news.test/",
            ResourceType::Script,
        );
        assert_eq!(ok, Decision::default());
    }

    #[test]
    fn exceptions_unblock_but_not_important_rules() {
        let f = filter();
        let d = f.check(
            "https://ads.example/allowed/x.js",
            "https://news.test/",
            ResourceType::Script,
        );
        assert!(!d.block && d.exception, "{d:?}");
        let d = f.check(
            "https://evil.test/ok",
            "https://news.test/",
            ResourceType::Script,
        );
        assert!(d.block && d.important, "{d:?}");
    }

    #[test]
    fn third_party_option() {
        let f = filter();
        // Loaded by another site: third party → blocked.
        assert!(
            f.check(
                "https://tracker.test/t.js",
                "https://news.test/",
                ResourceType::Script
            )
            .block
        );
        // Loaded by the tracker's own site: first party → allowed.
        assert!(
            !f.check(
                "https://tracker.test/t.js",
                "https://www.tracker.test/",
                ResourceType::Script
            )
            .block
        );
    }

    #[test]
    fn resource_type_options() {
        let f = filter();
        let page = "https://news.test/";
        assert!(
            f.check("https://img.test/banner/1.png", page, ResourceType::Image)
                .block
        );
        assert!(
            !f.check("https://img.test/banner/1.js", page, ResourceType::Script)
                .block
        );
        assert!(
            f.check("https://cdn.test/lib.js", page, ResourceType::Script)
                .block
        );
        assert!(
            !f.check("https://cdn.test/lib.js", page, ResourceType::Xhr)
                .block
        );
    }

    #[test]
    fn non_http_urls_are_never_blocked() {
        let f = filter();
        assert!(
            !f.check(
                "data:image/png;base64,AAAA",
                "https://news.test/",
                ResourceType::Image
            )
            .block
        );
        assert!(
            !f.check("bmcp://ui/index.html", "", ResourceType::MainFrame)
                .block
        );
    }

    #[test]
    fn cosmetic_rules_per_host() {
        let f = filter();
        let news = f.cosmetic("https://news.test/article");
        assert!(news.hide_selectors.contains(&".sponsored".to_string()));
        assert!(news.style.contains(".sponsored{display:none!important}"));
        // Generic class/id rules come from the page's classes and ids…
        let generic = f.generic_selectors(
            &["ad-banner".into(), "content".into()],
            &["top-ad".into()],
            &news.exceptions,
        );
        // …minus the `news.test#@#.ad-banner` exception.
        assert_eq!(generic, vec!["#top-ad".to_string()]);
        let other = f.cosmetic("https://other.test/");
        assert!(!other.hide_selectors.contains(&".sponsored".to_string()));
        let generic = f.generic_selectors(&["ad-banner".into()], &[], &other.exceptions);
        assert_eq!(generic, vec![".ad-banner".to_string()]);
    }

    #[test]
    fn serialization_round_trip() {
        let bytes = filter().serialize();
        assert!(!bytes.is_empty());
        let loaded = RequestFilter::from_serialized(&bytes).unwrap();
        assert!(
            loaded
                .check(
                    "https://ads.example/x",
                    "https://news.test/",
                    ResourceType::Script
                )
                .block
        );
        assert!(
            loaded
                .cosmetic("https://news.test/")
                .hide_selectors
                .contains(&".sponsored".to_string())
        );
        // Same bytes after a second round trip.
        assert_eq!(loaded.serialize(), bytes);
        assert!(RequestFilter::from_serialized(b"garbage").is_err());
    }

    #[test]
    fn resource_type_mappings() {
        assert_eq!(ResourceType::from_cef(0), ResourceType::MainFrame);
        assert_eq!(ResourceType::from_cef(13), ResourceType::Xhr);
        assert_eq!(ResourceType::from_cef(6), ResourceType::Other);
        assert_eq!(
            ResourceType::from_cdp("Document", false),
            ResourceType::SubFrame
        );
        assert_eq!(ResourceType::from_cdp("XHR", false), ResourceType::Xhr);
        assert_eq!(
            ResourceType::parse("XMLHttpRequest"),
            Some(ResourceType::Xhr)
        );
        assert_eq!(ResourceType::parse("nope"), None);
        assert_eq!(ResourceType::Fetch.adblock_name(), "xmlhttprequest");
    }

    #[test]
    fn host_and_domain_uses_public_suffixes() {
        assert_eq!(
            host_and_domain("https://cdn.ads.example.co.uk/x"),
            Some(("cdn.ads.example.co.uk".into(), "example.co.uk".into()))
        );
        assert_eq!(host_and_domain("not a url"), None);
    }
}
