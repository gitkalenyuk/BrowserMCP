//! Hint labels: the shortest prefix-free codes over a home-row alphabet.
//!
//! "Prefix-free" means no label is the start of another one, so a label is picked the moment
//! it is typed completely (no Enter needed): with `s` and `sa` both on screen, typing `s` would
//! be ambiguous.
//!
//! The codes are the leaves of a tree: start with the empty code and keep splitting the
//! shortest leaf into one child per alphabet letter until there are enough leaves. Splitting
//! the shortest leaf first keeps the labels as short as possible, and the first elements (top
//! of the page) get the shortest ones.

use std::collections::VecDeque;

/// Default alphabet: the home row, strongest fingers first.
pub const DEFAULT_ALPHABET: &str = "asdfghjkl";

/// `count` distinct, prefix-free labels over `alphabet` (which must have 2+ distinct letters;
/// see [`check_alphabet`]). Shorter labels come first.
pub fn labels(count: usize, alphabet: &str) -> Vec<String> {
    let letters: Vec<char> = alphabet.chars().collect();
    if count == 0 || letters.len() < 2 {
        return Vec::new();
    }
    // Leaves of the code tree, shortest first. Splitting the front leaf removes one leaf and
    // adds `letters.len()` longer ones at the back.
    let mut leaves: VecDeque<String> = VecDeque::from([String::new()]);
    while leaves.len() < count || leaves.front().is_some_and(String::is_empty) {
        let prefix = leaves.pop_front().expect("never empty");
        for &letter in &letters {
            let mut code = prefix.clone();
            code.push(letter);
            leaves.push_back(code);
        }
    }
    // Surplus leaves are the longest ones (at the back); drop them.
    leaves.into_iter().take(count).collect()
}

/// An alphabet is usable when it has at least two letters, all different, all lower-case
/// ASCII letters or digits (what a person types without Shift).
pub fn check_alphabet(alphabet: &str) -> Result<(), String> {
    let letters: Vec<char> = alphabet.chars().collect();
    if letters.len() < 2 {
        return Err("the alphabet needs at least two letters".into());
    }
    if let Some(bad) = letters
        .iter()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
    {
        return Err(format!(
            "alphabet letter {bad:?} is not a lower-case letter or digit"
        ));
    }
    for (i, c) in letters.iter().enumerate() {
        if letters[..i].contains(c) {
            return Err(format!("alphabet letter {c:?} appears twice"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix_free(codes: &[String]) -> bool {
        codes.iter().enumerate().all(|(i, a)| {
            codes
                .iter()
                .enumerate()
                .all(|(j, b)| i == j || !b.starts_with(a.as_str()))
        })
    }

    #[test]
    fn small_counts() {
        assert!(labels(0, DEFAULT_ALPHABET).is_empty());
        assert_eq!(labels(1, DEFAULT_ALPHABET), ["a"]);
        assert_eq!(labels(3, "asd"), ["a", "s", "d"]);
        // One more than the alphabet: the first letter becomes a prefix of two-letter codes.
        assert_eq!(labels(4, "asd"), ["s", "d", "aa", "as"]);
        assert_eq!(labels(5, "asd"), ["s", "d", "aa", "as", "ad"]);
    }

    #[test]
    fn unique_prefix_free_and_counted() {
        for n in [1, 2, 8, 9, 10, 80, 81, 82, 300, 1000] {
            let codes = labels(n, DEFAULT_ALPHABET);
            assert_eq!(codes.len(), n, "count for {n}");
            assert!(prefix_free(&codes), "prefix-free for {n}");
            assert!(
                codes
                    .iter()
                    .all(|c| c.chars().all(|ch| DEFAULT_ALPHABET.contains(ch)))
            );
        }
    }

    #[test]
    fn shortest_codes() {
        let k = DEFAULT_ALPHABET.len();
        for n in [2, 9, 10, 81, 82, 300, 729, 730] {
            let codes = labels(n, DEFAULT_ALPHABET);
            // The longest label is as short as possible: ceil(log_k n) letters.
            let mut need = 1;
            while k.pow(need) < n {
                need += 1;
            }
            let longest = codes.iter().map(String::len).max().unwrap();
            assert_eq!(longest, need as usize, "longest label for {n}");
            // Lengths never decrease: the first elements get the shortest labels.
            assert!(codes.windows(2).all(|w| w[0].len() <= w[1].len()));
        }
        // 10 hints over 9 letters: 8 single letters + 2 doubles is the minimum total (12).
        let total: usize = labels(10, DEFAULT_ALPHABET).iter().map(String::len).sum();
        assert_eq!(total, 12);
        // 300 hints: 9 letters can only do 81 two-letter codes, so 3-letter ones appear,
        // but most stay at 2 letters.
        let codes = labels(300, DEFAULT_ALPHABET);
        let two = codes.iter().filter(|c| c.len() == 2).count();
        assert!(two >= 50, "{two} two-letter labels");
    }

    #[test]
    fn alphabet_rules() {
        assert!(check_alphabet(DEFAULT_ALPHABET).is_ok());
        assert!(check_alphabet("a").is_err());
        assert!(check_alphabet("aA").is_err());
        assert!(check_alphabet("asa").is_err());
        assert!(check_alphabet("as;").is_err());
        assert!(labels(5, "a").is_empty());
    }
}
