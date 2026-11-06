//! What typing does while hints are shown: a small state machine over the typed letters.
//!
//! The page script reports every key (`KeyboardEvent.key`); [`Filter::key`] says what to do:
//!
//! * a letter that continues some label → [`KeyStep::Typed`] (the page dims the other labels);
//! * a letter that completes a label → [`KeyStep::Pick`] (labels are prefix-free, so a complete
//!   label is never the start of another one);
//! * a letter no label continues with → [`KeyStep::Ignore`] (the typo is dropped, like Vimium);
//! * `Backspace` → one letter back; `Escape` → [`KeyStep::Cancel`];
//! * `Enter` picks the first label still matching what was typed.

/// The outcome of one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStep {
    /// Nothing changes.
    Ignore,
    /// The typed prefix is now this (show only labels starting with it).
    Typed(String),
    /// The label with this index was chosen.
    Pick(usize),
    /// Hide the hints, do nothing.
    Cancel,
}

/// Labels on screen plus what was typed so far.
#[derive(Debug, Clone)]
pub struct Filter {
    labels: Vec<String>,
    typed: String,
}

impl Filter {
    pub fn new(labels: Vec<String>) -> Filter {
        Filter {
            labels,
            typed: String::new(),
        }
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// Index of the label `label`, if shown.
    pub fn index_of(&self, label: &str) -> Option<usize> {
        self.labels.iter().position(|l| l == label)
    }

    /// Feed one key (`KeyboardEvent.key`: `"a"`, `"S"`, `"Backspace"`, `"Escape"`…).
    pub fn key(&mut self, key: &str) -> KeyStep {
        match key {
            "Escape" | "Esc" => return KeyStep::Cancel,
            "Backspace" => {
                if self.typed.pop().is_some() {
                    return KeyStep::Typed(self.typed.clone());
                }
                return KeyStep::Ignore;
            }
            "Enter" => {
                return match self.labels.iter().position(|l| l.starts_with(&self.typed)) {
                    Some(i) if !self.typed.is_empty() => KeyStep::Pick(i),
                    _ => KeyStep::Ignore,
                };
            }
            _ => {}
        }
        // Only single characters type; Shift, Tab, arrows… are ignored. Labels are lower case,
        // so a letter typed with Shift (or Caps Lock) still counts.
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return KeyStep::Ignore;
        };
        let candidate = format!("{}{}", self.typed, c.to_ascii_lowercase());
        if let Some(i) = self.index_of(&candidate) {
            self.typed = candidate;
            return KeyStep::Pick(i);
        }
        if self.labels.iter().any(|l| l.starts_with(&candidate)) {
            self.typed = candidate;
            return KeyStep::Typed(self.typed.clone());
        }
        KeyStep::Ignore
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::labels;

    fn filter(labels: &[&str]) -> Filter {
        Filter::new(labels.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn single_letter_label_picks_at_once() {
        let mut f = filter(&["s", "d", "aa", "as"]);
        assert_eq!(f.key("d"), KeyStep::Pick(1));
    }

    #[test]
    fn two_letters_narrow_then_pick() {
        let mut f = filter(&["s", "d", "aa", "as"]);
        assert_eq!(f.key("a"), KeyStep::Typed("a".into()));
        assert_eq!(f.typed(), "a");
        assert_eq!(f.key("s"), KeyStep::Pick(3));
    }

    #[test]
    fn typos_are_dropped_and_case_is_ignored() {
        let mut f = filter(&["s", "d", "aa", "as"]);
        assert_eq!(f.key("A"), KeyStep::Typed("a".into()));
        // No label "ad": ignored, still at "a".
        assert_eq!(f.key("d"), KeyStep::Ignore);
        assert_eq!(f.typed(), "a");
        assert_eq!(f.key("x"), KeyStep::Ignore);
        assert_eq!(f.key("A"), KeyStep::Pick(2));
    }

    #[test]
    fn backspace_escape_and_other_keys() {
        let mut f = filter(&["s", "aa", "as"]);
        assert_eq!(f.key("Backspace"), KeyStep::Ignore);
        assert_eq!(f.key("a"), KeyStep::Typed("a".into()));
        assert_eq!(f.key("Backspace"), KeyStep::Typed(String::new()));
        assert_eq!(f.typed(), "");
        assert_eq!(f.key("Shift"), KeyStep::Ignore);
        assert_eq!(f.key("ArrowDown"), KeyStep::Ignore);
        assert_eq!(f.key("Escape"), KeyStep::Cancel);
    }

    #[test]
    fn enter_picks_the_first_match() {
        let mut f = filter(&["s", "aa", "as"]);
        assert_eq!(f.key("Enter"), KeyStep::Ignore);
        f.key("a");
        assert_eq!(f.key("Enter"), KeyStep::Pick(1));
    }

    #[test]
    fn every_generated_label_can_be_typed() {
        let all = labels(300, crate::labels::DEFAULT_ALPHABET);
        for (i, label) in all.iter().enumerate() {
            let mut f = Filter::new(all.clone());
            let mut last = KeyStep::Ignore;
            for c in label.chars() {
                last = f.key(&c.to_string());
            }
            assert_eq!(last, KeyStep::Pick(i), "label {label}");
        }
    }
}
