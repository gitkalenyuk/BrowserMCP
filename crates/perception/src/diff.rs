//! `diff`: what changed between two snapshots, in one line. Sent after actions instead of
//! a whole new snapshot to save tokens.
//!
//! ```text
//! diff 7→8: ~ @8:e6 button "Sign in" enabled | + alert "Wrong password" | - dialog "Cookies"
//! ```
//!
//! Nodes are matched by `backendDOMNodeId`. Only printed lines are compared, so diff two
//! snapshots taken with the same options (a skeleton hides most nodes).

use std::collections::{HashMap, HashSet};

use crate::snapshot::{NodeEntry, Snapshot};
use crate::text::quote;

/// More changes than this and the agent is better off with a new snapshot.
const MAX_CHANGES: usize = 30;

/// One-line description of the changes from `prev` to `next`.
pub fn diff(prev: &Snapshot, next: &Snapshot) -> String {
    let mut parts: Vec<String> = Vec::new();

    if prev.header.url != next.header.url {
        parts.push(format!("url {}", next.header.url));
    }
    if prev.header.title != next.header.title {
        parts.push(format!("title {}", quote(&next.header.title)));
    }

    let before: HashMap<i64, &NodeEntry> =
        prev.nodes.iter().map(|n| (n.backend_node_id, n)).collect();
    let after: HashSet<i64> = next.nodes.iter().map(|n| n.backend_node_id).collect();

    // Changed and added, in the new document order.
    for node in &next.nodes {
        match before.get(&node.backend_node_id) {
            None => parts.push(format!("+ {}", describe_added(node))),
            Some(old) => {
                let changes = changes(old, node);
                if !changes.is_empty() {
                    parts.push(format!("~ {} {}", label(node), changes.join(" ")));
                }
            }
        }
    }

    // Removed: only the top-most removed node (a removed dialog implies its buttons).
    let removed: HashSet<i64> = prev
        .nodes
        .iter()
        .map(|n| n.backend_node_id)
        .filter(|id| !after.contains(id))
        .collect();
    for node in &prev.nodes {
        if !removed.contains(&node.backend_node_id) {
            continue;
        }
        let parent_removed = node
            .parent_backend_node_id
            .is_some_and(|p| removed.contains(&p));
        if !parent_removed {
            parts.push(format!("- {}", role_and_name(node)));
        }
    }

    let head = format!("diff {}→{}:", prev.id, next.id);
    if parts.is_empty() {
        return format!("{head} no changes");
    }
    if parts.len() > MAX_CHANGES {
        let extra = parts.len() - MAX_CHANGES;
        parts.truncate(MAX_CHANGES);
        parts.push(format!("… {extra} more changes; take a new snapshot"));
    }
    format!("{head} {}", parts.join(" | "))
}

/// `button "Sign in"`
fn role_and_name(node: &NodeEntry) -> String {
    if node.name.is_empty() {
        node.role.clone()
    } else {
        format!("{} {}", node.role, quote(&node.name))
    }
}

/// `@8:e6 button "Sign in"` (ref first, so the agent sees what to act on).
fn label(node: &NodeEntry) -> String {
    match &node.ref_str {
        Some(r) => format!("{r} {}", role_and_name(node)),
        None => role_and_name(node),
    }
}

/// `alert "Wrong password"`, `button "Retry" @8:e9 disabled`
fn describe_added(node: &NodeEntry) -> String {
    let mut out = role_and_name(node);
    if let Some(r) = &node.ref_str {
        out.push(' ');
        out.push_str(r);
    }
    for state in visible_states(&node.states) {
        out.push(' ');
        out.push_str(state);
    }
    out
}

/// States that matter for a diff (`offscreen` flips on every scroll: too noisy).
fn visible_states(states: &[String]) -> impl Iterator<Item = &String> {
    states.iter().filter(|s| s.as_str() != "offscreen")
}

/// States from the same family replace each other (`off` -> `on`).
fn family(state: &str) -> &str {
    match state {
        "on" | "off" | "mixed" => "checked",
        "expanded" | "collapsed" => "expanded",
        s if s.starts_with('=') => "value",
        s if s.len() == 2 && s.starts_with('h') => "level",
        s => s,
    }
}

/// The word printed when a state goes away.
fn opposite(state: &str) -> String {
    match state {
        "disabled" => "enabled".to_string(),
        "invalid" => "valid".to_string(),
        "focused" => "blurred".to_string(),
        "selected" => "unselected".to_string(),
        "pressed" => "unpressed".to_string(),
        "readonly" => "editable".to_string(),
        s if s.starts_with('=') => "=\"\"".to_string(),
        s => format!("-{s}"),
    }
}

/// `["\"Sign in\"→\"Signing in…\"", "enabled"]`
fn changes(old: &NodeEntry, new: &NodeEntry) -> Vec<String> {
    let mut out = Vec::new();
    if old.role != new.role {
        out.push(format!("role {}→{}", old.role, new.role));
    }
    if old.name != new.name {
        out.push(format!("{}→{}", quote(&old.name), quote(&new.name)));
    }
    let old_states: Vec<&String> = visible_states(&old.states).collect();
    let new_states: Vec<&String> = visible_states(&new.states).collect();
    let added: Vec<&String> = new_states
        .iter()
        .copied()
        .filter(|s| !old_states.contains(s))
        .collect();
    let removed: Vec<&String> = old_states
        .iter()
        .copied()
        .filter(|s| !new_states.contains(s))
        .collect();
    for state in &added {
        out.push(state.to_string());
    }
    for state in removed {
        let replaced = added.iter().any(|a| family(a) == family(state));
        if !replaced {
            out.push(opposite(state));
        }
    }
    out
}
