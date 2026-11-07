//! URL helpers shared by the settings (pure).

use bmcp_core::CmdError;

/// Schemes a start page, home page or new tab page may use. `javascript:` and `data:` are
/// refused: a stored "page" must never be code that runs in whatever tab opens it.
const PAGE_SCHEMES: &[&str] = &["https", "http", "bmcp", "about", "file"];

/// A page URL as typed in Settings: `example.com` → `https://example.com/`, `localhost:3000`
/// → `http://localhost:3000/`; full URLs keep their scheme if it is allowed.
pub fn normalize_page_url(text: &str) -> Result<String, CmdError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CmdError::invalid_params("the URL is empty"));
    }
    if text.chars().any(char::is_whitespace) {
        return Err(CmdError::invalid_params(format!(
            "{text:?} is not a URL (it contains spaces)"
        )));
    }
    let (scheme, rest) = match text.split_once(':') {
        Some((scheme, rest))
            if !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
                && !rest.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
        {
            (scheme.to_ascii_lowercase(), rest)
        }
        // No scheme ("example.com", "localhost:3000"): a web page.
        _ => {
            let local = text.starts_with("localhost") || text.starts_with("127.");
            let scheme = if local { "http" } else { "https" };
            return Ok(with_root_path(&format!("{scheme}://{text}")));
        }
    };
    if !PAGE_SCHEMES.contains(&scheme.as_str()) {
        return Err(CmdError::invalid_params(format!(
            "{scheme}: URLs cannot be used here; use http(s)://, bmcp://, about: or file://"
        )));
    }
    if matches!(scheme.as_str(), "https" | "http") {
        let host = rest
            .trim_start_matches("//")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if host.is_empty() {
            return Err(CmdError::invalid_params(format!("{text:?} has no host")));
        }
    }
    Ok(with_root_path(&format!("{scheme}:{rest}")))
}

/// `https://a.test` → `https://a.test/` (what the browser shows once it loads).
fn with_root_path(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    if matches!(scheme, "http" | "https") && !rest.contains(['/', '?', '#']) {
        format!("{url}/")
    } else {
        url.to_string()
    }
}

/// Percent-encode like JavaScript's `encodeURIComponent` (the omnibox does the same, so a
/// search from the API and from the address bar give the same URL).
pub fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte);
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_urls_get_a_scheme_and_bad_schemes_are_refused() {
        assert_eq!(
            normalize_page_url("example.com").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            normalize_page_url(" https://a.test/x?q=1 ").unwrap(),
            "https://a.test/x?q=1"
        );
        assert_eq!(
            normalize_page_url("localhost:3000").unwrap(),
            "http://localhost:3000/"
        );
        assert_eq!(normalize_page_url("about:blank").unwrap(), "about:blank");
        assert_eq!(
            normalize_page_url("bmcp://ui/newtab.html").unwrap(),
            "bmcp://ui/newtab.html"
        );
        for bad in [
            "",
            "javascript:alert(1)",
            "data:text/html,x",
            "https://",
            "a b",
        ] {
            assert!(normalize_page_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(encode_component("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(encode_component("it's (ok)!*~"), "it's%20(ok)!*~");
    }
}
