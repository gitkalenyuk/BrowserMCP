//! `Fetch.RequestPattern` matching, so each client is only offered the pauses its own
//! patterns asked for.

use serde_json::Value;

/// Does a `Fetch.RequestPattern` (`{urlPattern?, resourceType?, requestStage?}`) match a pause?
pub(crate) fn pattern_matches(
    pattern: &Value,
    url: &str,
    resource_type: &str,
    response_stage: bool,
) -> bool {
    let stage_is_response = pattern["requestStage"].as_str() == Some("Response");
    if stage_is_response != response_stage {
        return false;
    }
    if let Some(wanted) = pattern["resourceType"].as_str()
        && wanted != resource_type
    {
        return false;
    }
    wildcard(pattern["urlPattern"].as_str().unwrap_or("*"), url)
}

#[derive(Clone, Copy, PartialEq)]
enum Tok {
    /// `*`: any run of characters.
    Star,
    /// `?`: exactly one character.
    One,
    Lit(char),
}

/// CDP wildcard match: `*` = any characters, `?` = one character, `\` escapes the next one.
pub(crate) fn wildcard(pattern: &str, text: &str) -> bool {
    let mut toks = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        toks.push(match c {
            '*' => Tok::Star,
            '?' => Tok::One,
            '\\' => match chars.next() {
                Some(next) => Tok::Lit(next),
                None => break,
            },
            other => Tok::Lit(other),
        });
    }
    let text: Vec<char> = text.chars().collect();
    // Classic greedy match that backtracks to the last `*` on a mismatch.
    let (mut p, mut t) = (0, 0);
    let mut last_star: Option<(usize, usize)> = None;
    while t < text.len() {
        match toks.get(p) {
            Some(Tok::One) => {
                p += 1;
                t += 1;
            }
            Some(Tok::Lit(c)) if *c == text[t] => {
                p += 1;
                t += 1;
            }
            Some(Tok::Star) => {
                last_star = Some((p, t));
                p += 1;
            }
            _ => match last_star {
                // Let the last `*` swallow one more character and try again.
                Some((sp, st)) => {
                    last_star = Some((sp, st + 1));
                    p = sp + 1;
                    t = st + 1;
                }
                None => return false,
            },
        }
    }
    toks[p..].iter().all(|tok| *tok == Tok::Star)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wildcards_match_like_cdp() {
        assert!(wildcard("*", ""));
        assert!(wildcard("*", "https://a.test/x"));
        assert!(wildcard("*://ads.test/*", "https://ads.test/banner.js"));
        assert!(!wildcard("*://ads.test/*", "https://a.test/ads.test/"));
        assert!(wildcard("*/api/*", "https://a.test/api/user"));
        assert!(!wildcard("*/api/*", "https://a.test/apix"));
        assert!(wildcard("https://a.test/?", "https://a.test/x"));
        assert!(!wildcard("https://a.test/?", "https://a.test/xy"));
        assert!(wildcard("*\\?q=*", "https://a.test/?q=1"));
        assert!(!wildcard("*\\?q=*", "https://a.test/xq=1"));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXbYY"));
    }

    #[test]
    fn patterns_check_stage_and_resource_type() {
        let req = json!({ "urlPattern": "*" });
        let res = json!({ "urlPattern": "*", "requestStage": "Response" });
        let xhr = json!({ "urlPattern": "*", "resourceType": "XHR" });
        assert!(pattern_matches(&req, "https://a.test/", "Script", false));
        assert!(!pattern_matches(&req, "https://a.test/", "Script", true));
        assert!(pattern_matches(&res, "https://a.test/", "Script", true));
        assert!(pattern_matches(&xhr, "https://a.test/", "XHR", false));
        assert!(!pattern_matches(&xhr, "https://a.test/", "Script", false));
    }
}
