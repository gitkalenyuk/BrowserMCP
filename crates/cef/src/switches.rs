//! Chromium command-line switches we add in the browser process.
//!
//! Why: when the browser window is covered by other windows, Chromium's native window
//! occlusion tracking marks the pages hidden, throttles them and silently drops synthetic
//! input (agents clicking via CDP got no effect). Two switches turn that off:
//! `--disable-features=CalculateNativeWinOcclusion` and
//! `--disable-backgrounding-occluded-windows`.

use cef::*;

/// Features we always disable (merged into any `--disable-features` the user passed).
const DISABLED_FEATURES: &[&str] = &["CalculateNativeWinOcclusion"];

/// Plain switches we always add.
const SWITCHES: &[&str] = &["disable-backgrounding-occluded-windows"];

/// Apply our switches to the browser process command line (`process_type` is empty for
/// the browser process; sub-processes inherit what they need from it).
pub(crate) fn apply(process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
    let is_browser = process_type.is_none_or(|t| t.to_string().is_empty());
    let Some(command_line) = command_line else {
        return;
    };
    if !is_browser {
        return;
    }
    let name = CefString::from("disable-features");
    let existing = (command_line.has_switch(Some(&name)) == 1)
        .then(|| CefString::from(&command_line.switch_value(Some(&name))).to_string());
    let merged = merge_feature_list(existing.as_deref(), DISABLED_FEATURES);
    // Chromium keeps the last value of a switch, so this replaces the user's value with
    // the merged list.
    command_line.append_switch_with_value(Some(&name), Some(&CefString::from(merged.as_str())));
    for switch in SWITCHES {
        command_line.append_switch(Some(&CefString::from(*switch)));
    }
}

/// `"A,B"` + `["B", "C"]` → `"A,B,C"` (keeps order, no duplicates, ignores empty items).
pub(crate) fn merge_feature_list(existing: Option<&str>, add: &[&str]) -> String {
    let mut items: Vec<&str> = existing
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    for feature in add {
        if !items.contains(feature) {
            items.push(feature);
        }
    }
    items.join(",")
}

#[cfg(test)]
mod tests {
    use super::merge_feature_list;

    #[test]
    fn merges_disable_features() {
        assert_eq!(merge_feature_list(None, &["X"]), "X");
        assert_eq!(merge_feature_list(Some(""), &["X"]), "X");
        assert_eq!(merge_feature_list(Some("A, B"), &["X"]), "A,B,X");
        assert_eq!(merge_feature_list(Some("X,A"), &["X"]), "X,A");
    }
}
