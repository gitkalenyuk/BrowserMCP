//! Small string helpers shared by the snapshot, find and diff code.

/// Longest accessible name we print, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest textbox value we print, in characters.
pub const MAX_VALUE_CHARS: usize = 40;

/// Collapse every run of whitespace (spaces, newlines, NBSP...) into one space and trim.
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cut `text` to at most `max` characters, adding `…` when something was cut.
/// Counts characters, not bytes, so it never splits a UTF-8 character.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Wrap in double quotes, escaping inner quotes: `say "hi"` -> `"say \"hi\""`.
pub fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\\\""))
}

/// Case-insensitive "does `haystack` contain `needle`".
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Case-insensitive equality.
pub fn eq_ci(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(clean("  Sign \n\t in\u{a0} "), "Sign in");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("абв", 3), "абв");
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert!(contains_ci("Sign In", "sign"));
        assert!(eq_ci("Button", "button"));
    }
}
