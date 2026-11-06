//! Preview formatter for `Accessibility.getFullAXTree` (spike S2/S3).
//!
//! Phase 3 replaces this with the real snapshot engine (refs, interactive-only,
//! skeleton/drill-down) in its own `perception` crate. This version only proves that
//! the accessibility tree gives an AI a usable picture of the page without pixels.

use std::collections::HashMap;
use std::fmt::Write;

use serde_json::Value;

/// Roles that only wrap other nodes; printed only if they carry a name.
const TRANSPARENT_ROLES: &[&str] = &[
    "generic",
    "none",
    "presentation",
    "InlineTextBox",
    "LineBreak",
    "RootWebArea",
    "group",
    "paragraph",
];

/// Render the AX tree as an indented `- role "name"` list.
pub fn format_ax_tree(result: &Value, max_lines: usize) -> String {
    let nodes = result
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| Some((n.get("nodeId")?.as_str()?, n)))
        .collect();
    let Some(root) = nodes.iter().find(|n| n.get("parentId").is_none()) else {
        return String::from("(empty accessibility tree)");
    };

    let mut out = String::new();
    let mut lines = 0;
    let mut stack: Vec<(&Value, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        let role = str_field(node, "role");
        let name = str_field(node, "name");
        let ignored = node
            .get("ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let printed = !ignored
            && !role.is_empty()
            && role != "StaticText"
            && role != "InlineTextBox"
            && (!TRANSPARENT_ROLES.contains(&role) || !name.is_empty());

        let child_depth = if printed {
            if lines == max_lines {
                let _ = writeln!(out, "… (truncated at {max_lines} lines)");
                break;
            }
            let indent = "  ".repeat(depth);
            if name.is_empty() {
                let _ = writeln!(out, "{indent}- {role}");
            } else {
                let _ = writeln!(out, "{indent}- {role} \"{}\"", truncate(name, 80));
            }
            lines += 1;
            depth + 1
        } else {
            depth
        };

        let children = node
            .get("childIds")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for child in children.iter().rev() {
            if let Some(child) = child.as_str().and_then(|id| by_id.get(id)) {
                stack.push((child, child_depth));
            }
        }
    }
    out
}

fn str_field<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get(key)
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: &str, parent: Option<&str>, role: &str, name: &str, children: &[&str]) -> Value {
        let mut n = json!({
            "nodeId": id,
            "ignored": false,
            "role": {"type": "role", "value": role},
            "name": {"type": "computedString", "value": name},
            "childIds": children,
        });
        if let Some(p) = parent {
            n["parentId"] = json!(p);
        }
        n
    }

    #[test]
    fn example_dot_com_shape() {
        let tree = json!({"nodes": [
            node("1", None, "RootWebArea", "Example Domain", &["2"]),
            node("2", Some("1"), "generic", "", &["3", "5"]),
            node("3", Some("2"), "heading", "Example Domain", &["4"]),
            node("4", Some("3"), "StaticText", "Example Domain", &[]),
            node("5", Some("2"), "paragraph", "", &["6"]),
            node("6", Some("5"), "link", "Learn more", &[]),
        ]});
        assert_eq!(
            format_ax_tree(&tree, 100),
            "- RootWebArea \"Example Domain\"\n  - heading \"Example Domain\"\n  - link \"Learn more\"\n"
        );
    }

    #[test]
    fn truncates_long_output() {
        let tree = json!({"nodes": [
            node("1", None, "list", "", &["2", "3", "4"]),
            node("2", Some("1"), "listitem", "a", &[]),
            node("3", Some("1"), "listitem", "b", &[]),
            node("4", Some("1"), "listitem", "c", &[]),
        ]});
        let out = format_ax_tree(&tree, 2);
        assert!(out.ends_with("… (truncated at 2 lines)\n"), "{out}");
    }

    #[test]
    fn empty_tree() {
        assert_eq!(format_ax_tree(&json!({}), 10), "(empty accessibility tree)");
    }
}
