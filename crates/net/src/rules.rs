//! Rules and breakpoints: which requests they match ([`Matcher`]) and what they do ([`Action`]).
//!
//! Both are applied through CDP's `Fetch` domain: `Fetch.enable` with URL patterns makes
//! Chromium pause matching requests (`Fetch.requestPaused`) until we answer with
//! `Fetch.continueRequest` / `fulfillRequest` / `failRequest` (see `intercept.rs`).

use std::collections::BTreeMap;

use bmcp_core::{CmdError, TabId};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// When a request is paused: before it is sent, or after the response headers arrived (the
/// body can then still be read and replaced).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    #[default]
    Request,
    Response,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Request => "request",
            Stage::Response => "response",
        }
    }

    /// The spelling `Fetch.enable` wants.
    fn cdp(self) -> &'static str {
        match self {
            Stage::Request => "Request",
            Stage::Response => "Response",
        }
    }
}

/// Which requests a rule or breakpoint applies to. All given parts must match.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    /// URL wildcard: `*` = any characters, `?` = one character, e.g. "*://api.example.com/*"
    /// or "*/items?*". Default: every URL.
    pub url_glob: Option<String>,
    /// Regular expression searched in the full URL, e.g. "/items/\\d+". Use instead of
    /// url_glob.
    pub url_regex: Option<String>,
    /// HTTP method such as "POST" (case-insensitive). Default: any.
    pub method: Option<String>,
    /// request (default) or response.
    pub stage: Option<Stage>,
}

/// A compiled [`MatchSpec`].
#[derive(Debug, Clone)]
pub struct Matcher {
    /// The wildcard pattern given to `Fetch.enable` ("*" when matching by regex).
    pub glob: String,
    /// Checked on every paused request (patterns of several rules overlap).
    regex: Regex,
    /// Uppercase HTTP method, `None` = any.
    pub method: Option<String>,
    pub stage: Stage,
    /// Human description, e.g. `POST glob "*/api/*"`.
    pub describe: String,
}

impl Matcher {
    /// Compile `spec`; `stage` (when given) wins over `spec.stage`.
    pub fn new(spec: &MatchSpec, stage: Option<Stage>) -> Result<Matcher, CmdError> {
        let stage = stage.or(spec.stage).unwrap_or_default();
        let method = spec.method.as_ref().map(|m| m.trim().to_ascii_uppercase());
        let (glob, regex, what) = match (&spec.url_glob, &spec.url_regex) {
            (Some(_), Some(_)) => {
                return Err(CmdError::invalid_params(
                    "give either match.url_glob or match.url_regex, not both",
                ));
            }
            (Some(glob), None) => (
                glob.clone(),
                glob_to_regex(glob),
                format!("glob {}", json!(glob)),
            ),
            (None, Some(re)) => {
                let regex = Regex::new(re).map_err(|e| {
                    CmdError::invalid_params(format!("match.url_regex is not a valid regex: {e}"))
                })?;
                ("*".to_string(), regex, format!("regex {}", json!(re)))
            }
            (None, None) => ("*".to_string(), glob_to_regex("*"), "any url".to_string()),
        };
        let describe = match &method {
            Some(m) => format!("{m} {what}"),
            None => what,
        };
        Ok(Matcher {
            glob,
            regex,
            method,
            stage,
            describe,
        })
    }

    pub fn matches(&self, url: &str, method: &str, stage: Stage) -> bool {
        stage == self.stage
            && self
                .method
                .as_ref()
                .is_none_or(|m| m.eq_ignore_ascii_case(method))
            && self.regex.is_match(url)
    }

    /// One entry of `Fetch.enable`'s `patterns`.
    pub fn pattern(&self) -> Value {
        json!({ "urlPattern": self.glob, "requestStage": self.stage.cdp() })
    }
}

/// CDP wildcard pattern → anchored regex. `*` = any run of characters, `?` = one character,
/// `\` escapes the next character (same rules as `Fetch.RequestPattern.urlPattern`).
pub fn glob_to_regex(glob: &str) -> Regex {
    let mut re = String::from("^");
    let mut chars = glob.chars();
    while let Some(c) = chars.next() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '\\' => {
                if let Some(next) = chars.next() {
                    re.push_str(&regex::escape(&next.to_string()));
                }
            }
            other => re.push_str(&regex::escape(&other.to_string())),
        }
    }
    re.push('$');
    // Only literal characters and `.*`/`.` are produced, so this always compiles.
    Regex::new(&re).expect("escaped glob is a valid regex")
}

/// What a rule does with a matching request.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Fail the request (the page sees net::ERR_BLOCKED_BY_CLIENT).
    Block,
    /// Send the request to another URL instead (the page does not notice). Request stage only.
    Redirect { url: String },
    /// Add or replace headers, e.g. {"x-debug": "1"}. Request headers at the request stage,
    /// response headers at the response stage.
    SetHeaders(BTreeMap<String, String>),
    /// Remove headers by name (case-insensitive), e.g. ["cookie"].
    RemoveHeaders(Vec<String>),
    /// Replace every occurrence of `find` in the body (request body at the request stage,
    /// response body at the response stage).
    RewriteBody { find: String, replace: String },
    /// Answer from here without touching the network.
    Mock {
        /// HTTP status (default 200).
        status: Option<u16>,
        /// Response headers; content-type is guessed from the body when missing.
        headers: Option<BTreeMap<String, String>>,
        /// Response body text (default empty).
        body: Option<String>,
    },
}

impl Action {
    /// Short name for notes and lists: `block`, `mock 404`, …
    pub fn short(&self) -> String {
        match self {
            Action::Block => "block".into(),
            Action::Redirect { url } => format!("redirect → {url}"),
            Action::SetHeaders(h) => format!(
                "set_headers {}",
                h.keys().cloned().collect::<Vec<_>>().join(",")
            ),
            Action::RemoveHeaders(h) => format!("remove_headers {}", h.join(",")),
            Action::RewriteBody { find, replace } => {
                format!("rewrite_body {} → {}", json!(find), json!(replace))
            }
            Action::Mock { status, .. } => format!("mock {}", status.unwrap_or(200)),
        }
    }
}

/// A rule or a breakpoint.
#[derive(Debug, Clone)]
pub enum RuleKind {
    Action(Action),
    /// Pause matching requests until `net.resume` (or 60 s).
    Breakpoint,
}

#[derive(Debug, Clone)]
pub struct Rule {
    /// `r3` for rules, `b4` for breakpoints.
    pub id: String,
    pub tab: TabId,
    pub matcher: Matcher,
    pub kind: RuleKind,
    /// How many requests it matched so far.
    pub hits: u64,
}

impl Rule {
    pub fn is_breakpoint(&self) -> bool {
        matches!(self.kind, RuleKind::Breakpoint)
    }

    /// `r1 tab 3 request GET glob "*/api/*" → block (2 hits)`.
    pub fn line(&self) -> String {
        let what = match &self.kind {
            RuleKind::Action(a) => a.short(),
            RuleKind::Breakpoint => "pause".into(),
        };
        format!(
            "{} tab {} {} {} → {} ({} hits)",
            self.id,
            self.tab,
            self.matcher.stage.as_str(),
            self.matcher.describe,
            what,
            self.hits
        )
    }
}

/// Check that an action makes sense at a stage.
pub fn validate(action: &Action, stage: Stage) -> Result<(), CmdError> {
    match action {
        Action::Redirect { .. } if stage == Stage::Response => Err(CmdError::invalid_params(
            "redirect only works at the request stage (the response already came from the \
             original URL)",
        )),
        Action::RewriteBody { find, .. } if find.is_empty() => Err(CmdError::invalid_params(
            "rewrite_body.find must not be empty",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_whole_urls() {
        let re = glob_to_regex("*://api.test/items?*");
        assert!(re.is_match("https://api.test/items?page=2"));
        assert!(!re.is_match("https://api.test/items"));
        assert!(!re.is_match("https://api.test/other?x"));
        let re = glob_to_regex("*.png");
        assert!(re.is_match("https://a.test/x.png"));
        assert!(!re.is_match("https://a.test/xpng"));
    }

    #[test]
    fn matcher_checks_stage_and_method() {
        let spec = MatchSpec {
            url_regex: Some(r"/items/\d+".into()),
            method: Some("post".into()),
            ..MatchSpec::default()
        };
        let m = Matcher::new(&spec, None).unwrap();
        assert_eq!(m.glob, "*");
        assert!(m.matches("https://a.test/items/7?x", "POST", Stage::Request));
        assert!(!m.matches("https://a.test/items/7", "GET", Stage::Request));
        assert!(!m.matches("https://a.test/items/7", "POST", Stage::Response));
        assert!(
            Matcher::new(
                &MatchSpec {
                    url_regex: Some("(".into()),
                    ..spec
                },
                None
            )
            .is_err()
        );
    }

    #[test]
    fn action_json_shapes() {
        let a: Action = serde_json::from_value(json!("block")).unwrap();
        assert_eq!(a, Action::Block);
        let a: Action = serde_json::from_value(json!({"remove_headers": ["cookie"]})).unwrap();
        assert_eq!(a, Action::RemoveHeaders(vec!["cookie".into()]));
        let a: Action =
            serde_json::from_value(json!({"mock": {"status": 404, "body": "nope"}})).unwrap();
        assert_eq!(a.short(), "mock 404");
    }
}
