//! `find`: search the whole accessibility tree (not just what a snapshot printed) and
//! return actionable refs. Cheaper than a snapshot when the agent knows what it wants.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::ax::{AxNode, AxTree};
use crate::error::{PerceptionError, Result};
use crate::refs::RefRegistry;
use crate::roles;
use crate::snapshot::descendant_text;
use crate::text::{MAX_NAME_CHARS, clean, contains_ci, eq_ci, quote, truncate};

/// What to look for. Every field that is set must match (AND).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Role, case-insensitive; CDP names and printed aliases both work
    /// (`contentinfo` or `footer`).
    pub role: Option<String>,
    /// Exact accessible name (case-insensitive, whitespace-normalized).
    pub name: Option<String>,
    /// Case-insensitive substring of the name, description or (non-password) value.
    /// Also finds plain text nodes, e.g. an error message.
    pub text: Option<String>,
    /// Regex matched against the name (use `(?i)` for case-insensitive).
    pub regex: Option<String>,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Match {
    /// `@9:e12`, or `None` when the node has no DOM node.
    pub ref_str: Option<String>,
    pub backend_node_id: Option<i64>,
    pub role: String,
    pub name: String,
    pub states: Vec<String>,
    /// Where it is: up to three enclosing containers, e.g. `main > form "Login"`.
    pub context: String,
}

/// Result of [`find`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindResult {
    /// Snapshot id the refs belong to (finds get ids like snapshots do).
    pub id: u64,
    /// Number of matches before applying `limit`.
    pub total: usize,
    pub matches: Vec<Match>,
}

impl FindResult {
    /// Compact text for the model:
    /// ```text
    /// find 9: 2 of 2 matches
    /// - button "Sign in" @9:e6 disabled in main > form "Login"
    /// ```
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "find {}: {} of {} matches",
            self.id,
            self.matches.len(),
            self.total
        );
        if self.total > self.matches.len() {
            out.push_str(" (narrow the filter to see the rest)");
        }
        for m in &self.matches {
            out.push_str("\n- ");
            out.push_str(&m.role);
            if !m.name.is_empty() {
                out.push(' ');
                out.push_str(&quote(&m.name));
            }
            if let Some(r) = &m.ref_str {
                out.push(' ');
                out.push_str(r);
            }
            for s in &m.states {
                out.push(' ');
                out.push_str(s);
            }
            if !m.context.is_empty() {
                out.push_str(" in ");
                out.push_str(&m.context);
            }
        }
        out
    }
}

/// Search `tree` for nodes matching `filter`; return at most `limit` matches in document
/// order (with `total` telling how many there were). Matched nodes get e-refs, so the
/// agent can act on them right away. Several matches for an exact name is the caller's
/// cue for `AMBIGUOUS_TARGET`.
pub fn find(
    tree: &AxTree,
    registry: &mut RefRegistry,
    filter: &Filter,
    limit: usize,
) -> Result<FindResult> {
    let is_set = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.trim().is_empty());
    if !(is_set(&filter.role)
        || is_set(&filter.name)
        || is_set(&filter.text)
        || is_set(&filter.regex))
    {
        return Err(PerceptionError::BadFilter(
            "give at least one of role, name, text, regex".to_string(),
        ));
    }
    let regex = match &filter.regex {
        Some(pattern) if !pattern.trim().is_empty() => {
            Some(Regex::new(pattern).map_err(|e| PerceptionError::BadFilter(e.to_string()))?)
        }
        _ => None,
    };

    let mut search = Search {
        tree,
        filter,
        regex,
        found: Vec::new(),
        visited: vec![false; tree.nodes.len()],
    };
    if let Some(root) = tree.root() {
        let mut ancestors = Vec::new();
        search.walk(root, &mut ancestors, "");
    }

    // Only the returned matches get refs (no point numbering 500 hidden hits).
    let total = search.found.len();
    let id = registry.begin_snapshot();
    let mut matches = Vec::new();
    for (index, context) in search.found.into_iter().take(limit) {
        let node = &tree.nodes[index];
        let ref_str = node
            .backend_dom_node_id
            .map(|b| format!("@{id}:e{}", registry.ids.number_for(b)));
        let interactive = roles::has_interactive_role(node) || roles::is_clickish_focusable(node);
        matches.push(Match {
            ref_str,
            backend_node_id: node.backend_dom_node_id,
            role: roles::display_role(node.role()),
            name: truncate(&clean(node.name()), MAX_NAME_CHARS),
            states: roles::states(tree, node, interactive),
            context,
        });
    }
    registry.finish_snapshot(id, Vec::new());
    Ok(FindResult { id, total, matches })
}

struct Search<'a> {
    tree: &'a AxTree,
    filter: &'a Filter,
    regex: Option<Regex>,
    /// (node position, context text)
    found: Vec<(usize, String)>,
    visited: Vec<bool>,
}

impl Search<'_> {
    /// Depth-first walk. `ancestors` holds context labels of enclosing containers;
    /// `context_name` is the nearest ancestor name (to skip text repeating it).
    fn walk(&mut self, index: usize, ancestors: &mut Vec<String>, context_name: &str) {
        if self.visited[index] {
            return;
        }
        self.visited[index] = true;
        let tree = self.tree;
        let node = &tree.nodes[index];
        let role = node.role();
        if roles::is_skipped(role) {
            return;
        }
        let name = clean(node.name());
        let real = !node.ignored && role != "RootWebArea";

        if real && self.matches(node, &name, context_name) {
            let start = ancestors.len().saturating_sub(3);
            self.found.push((index, ancestors[start..].join(" > ")));
        }

        let label = if real {
            context_label(tree, index, role, &name)
        } else {
            None
        };
        let pushed = label.is_some();
        if let Some(label) = label {
            ancestors.push(label);
        }
        let child_context = if real && !name.is_empty() {
            name.clone()
        } else {
            context_name.to_string()
        };
        for child in tree.children(index) {
            self.walk(child, ancestors, &child_context);
        }
        if pushed {
            ancestors.pop();
        }
    }

    fn matches(&self, node: &AxNode, name: &str, context_name: &str) -> bool {
        let role = node.role();
        // Text that just repeats its parent's name (a button's label) is not a new hit.
        if role == "StaticText" && (name.is_empty() || contains_ci(context_name, name)) {
            return false;
        }
        if let Some(want) = non_empty(&self.filter.role)
            && !roles::role_matches(want, role)
        {
            return false;
        }
        if let Some(want) = non_empty(&self.filter.name)
            && !eq_ci(name, &clean(want))
        {
            return false;
        }
        if let Some(want) = non_empty(&self.filter.text) {
            let want = clean(want);
            // Never search inside password values.
            let value = if roles::is_password(self.tree, node) {
                String::new()
            } else {
                node.value_text()
            };
            let hit = contains_ci(name, &want)
                || contains_ci(&value, &want)
                || contains_ci(node.description(), &want);
            if !hit {
                return false;
            }
        }
        if let Some(regex) = &self.regex
            && !regex.is_match(name)
        {
            return false;
        }
        true
    }
}

fn non_empty(option: &Option<String>) -> Option<&str> {
    option.as_deref().filter(|s| !s.trim().is_empty())
}

/// How an ancestor is described in `context`: `main`, `form "Login"`,
/// `listitem "Product 7 $17.99 Add to cart"`. `None` for uninteresting wrappers.
fn context_label(tree: &AxTree, index: usize, role: &str, name: &str) -> Option<String> {
    let grouping = roles::is_grouping(role) || role == "row";
    if !(roles::is_structure(role, name) || grouping) {
        return None;
    }
    let display = roles::display_role(role);
    let shown = if !name.is_empty() {
        name.to_string()
    } else if grouping {
        // Unnamed cards/rows are told apart by their text.
        descendant_text(tree, index)
    } else {
        String::new()
    };
    if shown.is_empty() {
        Some(display)
    } else {
        Some(format!("{display} {}", quote(&truncate(&shown, 40))))
    }
}
