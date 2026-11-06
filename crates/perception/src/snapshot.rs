//! The snapshot engine: accessibility tree -> compact text with refs.
//!
//! It works in two passes:
//! 1. **Build**: walk the CDP tree and produce a small "view tree" ([`VNode`]) that
//!    keeps only what the agent needs (controls, headings, landmarks around them) and
//!    flattens wrappers (`generic`, ignored nodes, unnamed forms...).
//! 2. **Render**: print the view tree as indented lines, hand out refs, collapse long
//!    lists into group refs, and fall back to a skeleton when the text is over budget.
//!
//! Rendering runs on a *copy* of the ref tables and only commits the copy when the final
//! text is chosen, so refs from discarded attempts never leak into the registry.

use serde::{Deserialize, Serialize};

use crate::ax::AxTree;
use crate::error::{PerceptionError, Result};
use crate::refs::{ElementIds, GroupTarget, RefKind, RefRegistry, parse_ref};
use crate::roles;
use crate::text::{MAX_NAME_CHARS, clean, contains_ci, quote, truncate};

/// How much of the page to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Controls (with refs) plus the headings/landmarks around them. Text is dropped.
    #[default]
    Interactive,
    /// Everything meaningful, including text. Wrappers are still flattened.
    Full,
    /// Interactive content, but only `depth` levels deep; deeper parts become
    /// `(collapsed) → @S:gN` lines the agent can drill into.
    Skeleton { depth: usize },
}

/// Options for [`snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotOptions {
    pub mode: Mode,
    /// Drill down: start at this ref (`@7:g1` group or `@7:e3` element) instead of the
    /// page root.
    pub root: Option<String>,
    /// Character budget for the text (~4 chars per token). Over budget, the snapshot
    /// falls back to a skeleton automatically.
    pub max_chars: usize,
    /// Lists with more children than this collapse...
    pub collapse_lists_over: usize,
    /// ...showing only this many of them.
    pub collapse_show: usize,
}

impl Default for SnapshotOptions {
    fn default() -> SnapshotOptions {
        SnapshotOptions {
            mode: Mode::Interactive,
            root: None,
            max_chars: 6000,
            collapse_lists_over: 8,
            collapse_show: 3,
        }
    }
}

/// Viewport size in CSS pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// Vertical scroll position: `y` = current scroll offset, `height` = document height.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scroll {
    pub y: u32,
    pub height: u32,
}

/// Page facts printed in the first line of the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageHeader {
    pub tab: u32,
    pub title: String,
    pub url: String,
    pub viewport: Option<Viewport>,
    pub scroll: Option<Scroll>,
}

/// One printed ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefEntry {
    /// As printed, e.g. `@7:e3`.
    pub ref_str: String,
    pub backend_node_id: i64,
    pub role: String,
    pub name: String,
}

/// One printed line that belongs to a DOM node (used by [`crate::diff`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeEntry {
    pub backend_node_id: i64,
    /// Nearest printed ancestor with a DOM node.
    pub parent_backend_node_id: Option<i64>,
    pub ref_str: Option<String>,
    pub role: String,
    pub name: String,
    pub states: Vec<String>,
}

/// The result of [`snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    /// Snapshot id (the `7` in `@7:e3`), increasing and unique across all tabs whose
    /// registries share a [`crate::RefSpace`].
    pub id: u64,
    /// Document generation of the registry when this was taken.
    pub document: u64,
    pub header: PageHeader,
    /// The text for the model.
    pub text: String,
    /// Every printed ref, in document order.
    pub refs: Vec<RefEntry>,
    /// Every printed node that has a DOM node, in document order.
    pub nodes: Vec<NodeEntry>,
    /// True when the text was over budget and a skeleton is shown instead.
    pub skeleton_fallback: bool,
    /// True when lines were left out: even the smallest skeleton was over budget, or the
    /// page tree is deeper than [`MAX_TREE_DEPTH`].
    pub cut: bool,
}

/// How deep into the accessibility tree snapshots and `find` look. The walk is
/// recursive, so a page that nests elements thousands of levels deep (a script can do
/// that) would otherwise overflow the stack and abort the whole app. Real pages stay far
/// below this; deeper parts are left out with a `! ...` notice.
pub const MAX_TREE_DEPTH: usize = 512;

/// A node of the pruned "view tree".
#[derive(Debug, Clone)]
struct VNode {
    /// Position of the AX node in `AxTree::nodes`.
    ax: usize,
    /// Printed role (after aliasing, e.g. `footer`).
    role: String,
    /// Cleaned and truncated name.
    name: String,
    states: Vec<String>,
    /// Gets a ref.
    interactive: bool,
    backend: Option<i64>,
    children: Vec<VNode>,
}

/// Pass 1: AX tree -> view tree.
struct Builder<'a> {
    tree: &'a AxTree,
    full: bool,
    /// Protects against malformed trees with cycles.
    visited: Vec<bool>,
    /// Set when part of the tree was deeper than [`MAX_TREE_DEPTH`] and was left out.
    too_deep: bool,
}

impl<'a> Builder<'a> {
    fn new(tree: &'a AxTree, full: bool) -> Builder<'a> {
        Builder {
            tree,
            full,
            visited: vec![false; tree.nodes.len()],
            too_deep: false,
        }
    }

    /// Build all children of `index` (which is `depth` levels below the start), flattening
    /// their results into one list. `context_name` is the name of the nearest named
    /// ancestor (used to drop text that only repeats it).
    fn build_children(&mut self, index: usize, context_name: &str, depth: usize) -> Vec<VNode> {
        let mut out = Vec::new();
        for child in self.tree.children(index) {
            out.extend(self.build(child, context_name, depth + 1));
        }
        if self.full {
            drop_label_texts(&mut out);
        }
        out
    }

    /// Build one AX node, `depth` levels below the start. Returns zero nodes (dropped),
    /// one node (kept) or the node's children (flattened).
    fn build(&mut self, index: usize, context_name: &str, depth: usize) -> Vec<VNode> {
        if self.visited[index] {
            return Vec::new();
        }
        // The recursion stops here, so the view tree (and everything that walks it
        // recursively later, including dropping it) is at most this deep too.
        if depth > MAX_TREE_DEPTH {
            self.too_deep = true;
            return Vec::new();
        }
        self.visited[index] = true;

        let tree = self.tree;
        let node = &tree.nodes[index];
        let role = node.role();
        if roles::is_skipped(role) {
            return Vec::new();
        }
        if node.ignored {
            return self.build_children(index, context_name, depth);
        }
        if role == "StaticText" {
            return self.build_text(index, context_name);
        }

        let mut name = clean(node.name());
        let child_context: &str = if name.is_empty() { context_name } else { &name };
        let mut children = self.build_children(index, child_context, depth);

        // A focusable div that *contains* buttons is a scroll container, not a button.
        let interactive = roles::has_interactive_role(node)
            || (roles::is_clickish_focusable(node) && !any_interactive(&children));
        let always = roles::is_always_kept(role);
        if name.is_empty() && (interactive || always) {
            // e.g. `<div role=alert>Wrong password</div>` has no name, only text.
            name = descendant_text(tree, index);
            // Full mode: the text children now only repeat that name.
            children.retain(|c| !(is_text_leaf(c) && contains_ci(&name, &c.name)));
        }
        if interactive && matches!(role, "textbox" | "searchbox" | "combobox" | "spinbutton") {
            // Text inside a field is its value (`•••••` for passwords): states show it.
            children.retain(|c| !is_text_leaf(c));
        }
        let has_content = !name.is_empty() || !children.is_empty();

        let keep = if interactive {
            true
        } else if always {
            has_content
        } else if self.full {
            // Unnamed wrappers go, and so do unnamed plain containers around a single
            // child (`- listitem` around one button adds nothing).
            let lone_child = name.is_empty()
                && children.len() == 1
                && !is_text_leaf(&children[0])
                && !roles::is_structure(role, &name);
            has_content && !(roles::is_wrapper(role) && name.is_empty()) && !lone_child
        } else if roles::is_structure(role, &name) {
            !children.is_empty()
        } else if roles::is_grouping(role) {
            children.len() >= 2
        } else {
            false
        };
        if !keep {
            return children;
        }

        // Full mode: fold a lone text child into its parent: `- paragraph "Hello"`.
        if name.is_empty() && children.len() == 1 && is_text_leaf(&children[0]) {
            name = children.remove(0).name;
        }

        vec![VNode {
            ax: index,
            role: roles::display_role(role),
            name: truncate(&name, MAX_NAME_CHARS),
            states: roles::states(tree, node, interactive),
            interactive,
            backend: node.backend_dom_node_id,
            children,
        }]
    }

    /// Text nodes: only in full mode, and only when they add something new.
    fn build_text(&mut self, index: usize, context_name: &str) -> Vec<VNode> {
        if !self.full {
            return Vec::new();
        }
        let node = &self.tree.nodes[index];
        let text = clean(node.name());
        if text.is_empty() || contains_ci(context_name, &text) {
            return Vec::new();
        }
        vec![VNode {
            ax: index,
            role: "text".to_string(),
            name: truncate(&text, MAX_NAME_CHARS),
            states: Vec::new(),
            interactive: false,
            backend: node.backend_dom_node_id,
            children: Vec::new(),
        }]
    }
}

/// All visible text under a node, joined with spaces (stops after ~80 chars).
/// Only text nodes are read, never input values, so passwords cannot leak.
pub(crate) fn descendant_text(tree: &AxTree, index: usize) -> String {
    let mut parts = Vec::new();
    collect_text(tree, index, &mut parts, 0);
    clean(&parts.join(" "))
}

fn collect_text(tree: &AxTree, index: usize, parts: &mut Vec<String>, depth: usize) {
    let length: usize = parts.iter().map(|p| p.len() + 1).sum();
    if depth > 64 || length > MAX_NAME_CHARS {
        return;
    }
    for child in tree.children(index) {
        let node = &tree.nodes[child];
        if node.role() == "StaticText" {
            let text = clean(node.name());
            if !node.ignored && !text.is_empty() {
                parts.push(text);
            }
        } else if !roles::is_skipped(node.role()) {
            collect_text(tree, child, parts, depth + 1);
        }
    }
}

fn is_text_leaf(node: &VNode) -> bool {
    node.role == "text" && node.children.is_empty()
}

/// Full mode: drop text that only repeats the name of a sibling control, which is what
/// `<label>Email</label><input>` turns into.
fn drop_label_texts(nodes: &mut Vec<VNode>) {
    let control_names: Vec<String> = nodes
        .iter()
        .filter(|n| n.interactive)
        .map(|n| n.name.to_lowercase())
        .collect();
    nodes.retain(|n| !(is_text_leaf(n) && control_names.contains(&n.name.to_lowercase())));
}

fn count_nodes(nodes: &[VNode]) -> usize {
    nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        format!("{count} {word}s")
    }
}

fn any_interactive(nodes: &[VNode]) -> bool {
    nodes
        .iter()
        .any(|n| n.interactive || any_interactive(&n.children))
}

fn count_interactive(nodes: &[VNode]) -> usize {
    nodes
        .iter()
        .map(|n| usize::from(n.interactive) + count_interactive(&n.children))
        .sum()
}

/// Pass 2: view tree -> lines. One `Renderer` is one attempt at the final text.
struct Renderer<'a> {
    tree: &'a AxTree,
    snap_id: u64,
    /// Working copy of the registry's e-number tables.
    ids: ElementIds,
    groups: Vec<GroupTarget>,
    lines: Vec<String>,
    /// Characters so far, counting one newline per line.
    chars: usize,
    refs: Vec<RefEntry>,
    nodes: Vec<NodeEntry>,
    /// Skeleton mode: collapse everything at this depth.
    skeleton_depth: Option<usize>,
    collapse_over: usize,
    collapse_show: usize,
    /// The drill-down root is never list-collapsed (that is the point of drilling).
    expand_ax: Option<usize>,
    /// Hard limit; once a line does not fit, everything after it is cut.
    budget: Option<usize>,
    cut: bool,
}

impl Renderer<'_> {
    /// Add a line unless we are over the hard budget. Returns false when cut.
    fn push_line(&mut self, line: String) -> bool {
        if self.cut {
            return false;
        }
        let size = line.chars().count() + 1;
        if let Some(budget) = self.budget
            && self.chars + size > budget
        {
            self.cut = true;
            return false;
        }
        self.chars += size;
        self.lines.push(line);
        true
    }

    fn render_all(&mut self, nodes: &[VNode], depth: usize, parent: Option<i64>) {
        for node in nodes {
            self.render_node(node, depth, parent);
        }
    }

    fn render_node(&mut self, node: &VNode, depth: usize, parent: Option<i64>) {
        // "  - textbox "Email" @7:e3 req"
        let mut line = format!("{}- {}", "  ".repeat(depth), node.role);
        if !node.name.is_empty() {
            line.push(' ');
            line.push_str(&quote(&node.name));
        }
        let mut ref_str = None;
        if node.interactive
            && let Some(backend) = node.backend
        {
            let number = self.ids.number_for(backend);
            let text = format!("@{}:e{}", self.snap_id, number);
            line.push(' ');
            line.push_str(&text);
            ref_str = Some(text);
        }
        for state in &node.states {
            line.push(' ');
            line.push_str(state);
        }

        // Which children to print.
        let mut shown: &[VNode] = &node.children;
        if !node.children.is_empty() {
            // Tiny subtrees (one or two lines) are cheaper to show than to collapse.
            let at_skeleton_limit = self.skeleton_depth.is_some_and(|d| depth + 1 >= d)
                && count_nodes(&node.children) > 2;
            if at_skeleton_limit {
                let inside = count_interactive(&node.children);
                let group = self.add_group(node);
                if inside > 0 {
                    let refs = plural(inside, "ref");
                    line.push_str(&format!(" (collapsed, {refs}) → {group}"));
                } else {
                    line.push_str(&format!(" (collapsed) → {group}"));
                }
                shown = &[];
            } else if roles::is_collapsible(&node.role)
                && node.children.len() > self.collapse_over
                && self.expand_ax != Some(node.ax)
            {
                let group = self.add_group(node);
                let count = node.children.len();
                let show = self.collapse_show.min(count);
                line.push_str(&format!(" {count} items, {show} shown → {group}"));
                shown = &node.children[..show];
            }
        }

        if !self.push_line(line) {
            return;
        }
        if let Some(backend) = node.backend {
            if let Some(text) = &ref_str {
                self.refs.push(RefEntry {
                    ref_str: text.clone(),
                    backend_node_id: backend,
                    role: node.role.clone(),
                    name: node.name.clone(),
                });
            }
            self.nodes.push(NodeEntry {
                backend_node_id: backend,
                parent_backend_node_id: parent,
                ref_str,
                role: node.role.clone(),
                name: node.name.clone(),
                states: node.states.clone(),
            });
        }
        self.render_all(shown, depth + 1, node.backend.or(parent));
    }

    /// Remember a collapsed container and return its printed ref (`@7:g1`).
    fn add_group(&mut self, node: &VNode) -> String {
        self.groups.push(GroupTarget {
            backend_node_id: node.backend,
            ax_node_id: self.tree.nodes[node.ax].node_id.clone(),
            role: node.role.clone(),
            name: node.name.clone(),
        });
        format!("@{}:g{}", self.snap_id, self.groups.len())
    }
}

/// Everything one render attempt needs besides the depth/notice/budget.
struct Plan<'a> {
    tree: &'a AxTree,
    roots: &'a [VNode],
    snap_id: u64,
    header: String,
    /// Printed under the header in every attempt (the tree-depth notice).
    fixed_notice: Option<String>,
    options: &'a SnapshotOptions,
    expand_ax: Option<usize>,
}

fn render_attempt<'a>(
    plan: &Plan<'a>,
    registry: &RefRegistry,
    skeleton_depth: Option<usize>,
    notice: Option<String>,
    budget: Option<usize>,
) -> Renderer<'a> {
    let mut renderer = Renderer {
        tree: plan.tree,
        snap_id: plan.snap_id,
        ids: registry.ids.clone(),
        groups: Vec::new(),
        lines: Vec::new(),
        chars: 0,
        refs: Vec::new(),
        nodes: Vec::new(),
        skeleton_depth,
        collapse_over: plan.options.collapse_lists_over,
        collapse_show: plan.options.collapse_show,
        expand_ax: plan.expand_ax,
        budget: None,
        cut: false,
    };
    // The header line (and the depth notice) is always printed; the budget applies to
    // everything after it.
    renderer.push_line(plan.header.clone());
    if let Some(notice) = &plan.fixed_notice {
        renderer.push_line(notice.clone());
    }
    renderer.budget = budget;
    if let Some(notice) = notice {
        renderer.push_line(notice);
    }
    let fixed_lines = renderer.lines.len();
    renderer.render_all(plan.roots, 0, None);
    if renderer.lines.len() == fixed_lines && !renderer.cut {
        let hint = match plan.options.mode {
            Mode::Full => "- (empty)",
            _ => "- (nothing interactive here; try mode=full)",
        };
        renderer.push_line(hint.to_string());
    }
    renderer
}

/// Take a snapshot.
///
/// * `tree`: the parsed `Accessibility.getFullAXTree` result.
/// * `registry`: this tab's ref registry (refs are assigned and remembered here).
/// * `options`: mode, drill-down root, budget, list collapsing.
/// * `header`: page facts for the first line.
///
/// Errors only when `options.root` is a bad/stale ref or not in the tree, or the tree
/// is empty.
pub fn snapshot(
    tree: &AxTree,
    registry: &mut RefRegistry,
    options: &SnapshotOptions,
    header: PageHeader,
) -> Result<Snapshot> {
    // Where to start: the page root, or the node a drill-down ref points at.
    let root = match &options.root {
        None => tree.root().ok_or_else(|| {
            PerceptionError::NotFound("the accessibility tree is empty".to_string())
        })?,
        Some(input) => find_root(tree, registry, input)?,
    };

    let mut builder = Builder::new(tree, options.mode == Mode::Full);
    let roots = if tree.nodes[root].role() == "RootWebArea" {
        // The document itself is the header line; print its children at depth 0.
        builder.build_children(root, "", 0)
    } else {
        builder.build(root, "", 0)
    };
    let fixed_notice = builder.too_deep.then(|| {
        format!(
            "! the page tree is deeper than {MAX_TREE_DEPTH} levels; deeper parts are not shown"
        )
    });

    // From here on this snapshot hands out snapshot ids and e-numbers: hold the shared
    // counters so a snapshot of another tab cannot hand out the same ones meanwhile.
    let space = registry.space();
    let mut counters = space.lock();
    let id = registry.begin_snapshot(&mut counters);
    let plan = Plan {
        tree,
        roots: &roots,
        snap_id: id,
        header: header_line(id, &header, options),
        fixed_notice,
        options,
        expand_ax: options.root.as_ref().map(|_| root),
    };
    let max = options.max_chars;
    let requested_depth = match options.mode {
        Mode::Skeleton { depth } => Some(depth.max(1)),
        _ => None,
    };

    // First try exactly what was asked for.
    let mut depth = requested_depth;
    let mut notice = None;
    let mut result = render_attempt(&plan, registry, depth, None, None);

    // Over budget: try shallower skeletons, then cut lines as a last resort.
    if result.chars > max {
        let original = result.chars;
        let mut next_depth = requested_depth.map(|d| d - 1).unwrap_or(3);
        while next_depth >= 1 && result.chars > max {
            depth = Some(next_depth);
            notice = Some(format!(
                "! {original} chars is over the {max} budget: skeleton depth {next_depth} shown; \
                 drill down with root=@{id}:gN or use find"
            ));
            result = render_attempt(&plan, registry, depth, notice.clone(), None);
            next_depth -= 1;
        }
        if result.chars > max {
            let cut_line =
                format!("… cut to fit {max} chars; drill down with root=@{id}:gN or use find");
            let reserve = cut_line.chars().count() + 1;
            result = render_attempt(
                &plan,
                registry,
                depth,
                notice,
                Some(max.saturating_sub(reserve)),
            );
            result.lines.push(cut_line);
        }
    }

    // Commit the chosen attempt.
    let skeleton_fallback = depth != requested_depth;
    let cut = result.cut || builder.too_deep;
    let text = result.lines.join("\n");
    registry.ids = result.ids;
    registry.finish_snapshot(&mut counters, id, result.groups);
    drop(counters);
    Ok(Snapshot {
        id,
        document: registry.document(),
        header,
        text,
        refs: result.refs,
        nodes: result.nodes,
        skeleton_fallback,
        cut,
    })
}

/// `snap 7 tab=3 "Sign in – Acme" https://acme.com/login vp=1280x720 y=0/1840`
fn header_line(id: u64, header: &PageHeader, options: &SnapshotOptions) -> String {
    let title = quote(&truncate(&clean(&header.title), MAX_NAME_CHARS));
    let mut line = format!(
        "snap {id} tab={} {title} {}",
        header.tab,
        truncate(&header.url, 120)
    );
    if let Some(vp) = &header.viewport {
        line.push_str(&format!(" vp={}x{}", vp.width, vp.height));
    }
    if let Some(scroll) = &header.scroll {
        line.push_str(&format!(" y={}/{}", scroll.y, scroll.height));
    }
    match options.mode {
        Mode::Interactive => {}
        Mode::Full => line.push_str(" mode=full"),
        Mode::Skeleton { depth } => line.push_str(&format!(" mode=skeleton:{depth}")),
    }
    if let Some(root) = &options.root {
        line.push_str(&format!(" root={}", root.trim()));
    }
    line
}

/// Turn a drill-down ref into a position in the tree.
fn find_root(tree: &AxTree, registry: &RefRegistry, input: &str) -> Result<usize> {
    let not_found = || {
        PerceptionError::NotFound(format!(
            "{input} is not in the page any more; take a new snapshot"
        ))
    };
    match parse_ref(input)?.kind {
        RefKind::Element => {
            let backend = registry.resolve(input)?;
            tree.index_of_backend(backend).ok_or_else(not_found)
        }
        RefKind::Group => {
            let group = registry.resolve_group(input)?;
            group
                .backend_node_id
                .and_then(|b| tree.index_of_backend(b))
                .or_else(|| tree.index_of_node_id(&group.ax_node_id))
                .ok_or_else(not_found)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ax::NodeLayout;

    /// Tiny tree builder for edge cases: (id, parent, role, name, properties, backend).
    fn tree(rows: &[(&str, &str, &str, &str, &str, i64)]) -> AxTree {
        let nodes: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, parent, role, name, props, backend)| {
                let children: Vec<&str> = rows
                    .iter()
                    .filter(|r| r.1 == *id)
                    .map(|r| r.0)
                    .collect();
                let properties: Vec<serde_json::Value> = props
                    .split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| serde_json::json!({"name": p, "value": {"type": "boolean", "value": true}}))
                    .collect();
                let mut node = serde_json::json!({
                    "nodeId": id, "role": {"type": "role", "value": role},
                    "name": {"type": "computedString", "value": name},
                    "properties": properties, "childIds": children, "backendDOMNodeId": backend,
                });
                if !parent.is_empty() {
                    node["parentId"] = serde_json::json!(parent);
                }
                node
            })
            .collect();
        AxTree::from_cdp(&serde_json::json!({ "nodes": nodes })).unwrap()
    }

    fn text_of(tree: &AxTree) -> String {
        let mut registry = RefRegistry::new();
        let snap = snapshot(
            tree,
            &mut registry,
            &SnapshotOptions::default(),
            PageHeader::default(),
        )
        .unwrap();
        snap.text.lines().skip(1).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn focusable_div_is_a_button_but_a_focusable_scroller_is_not() {
        let t = tree(&[
            ("1", "", "RootWebArea", "", "", 1),
            ("2", "1", "generic", "", "focusable", 2),
            ("3", "2", "StaticText", "Open menu", "", 3),
            ("4", "1", "generic", "", "focusable", 4),
            ("5", "4", "button", "Inside scroller", "focusable", 5),
        ]);
        assert_eq!(
            text_of(&t),
            "- generic \"Open menu\" @1:e1\n- button \"Inside scroller\" @1:e2"
        );
    }

    #[test]
    fn empty_live_regions_are_dropped_and_messages_kept() {
        let t = tree(&[
            ("1", "", "RootWebArea", "", "", 1),
            ("2", "1", "alert", "", "", 2),
            ("3", "1", "status", "", "", 3),
            ("4", "3", "StaticText", "Saved", "", 4),
        ]);
        assert_eq!(text_of(&t), "- status \"Saved\"");
    }

    #[test]
    fn offscreen_from_layout() {
        let t = tree(&[
            ("1", "", "RootWebArea", "", "", 1),
            ("2", "1", "button", "Below the fold", "", 2),
        ]);
        let layout = NodeLayout {
            in_viewport: Some(false),
            ..NodeLayout::default()
        };
        let t = t.with_layout(HashMap::from([(2, layout)]));
        assert_eq!(text_of(&t), "- button \"Below the fold\" @1:e1 offscreen");
    }

    #[test]
    fn full_mode_drops_field_values_and_repeated_alert_text() {
        let t = tree(&[
            ("1", "", "RootWebArea", "", "", 1),
            ("2", "1", "textbox", "Password", "", 2),
            ("3", "2", "StaticText", "•••••", "", 3),
            ("4", "1", "alert", "", "", 4),
            ("5", "4", "StaticText", "Wrong password", "", 5),
        ]);
        let mut registry = RefRegistry::new();
        let options = SnapshotOptions {
            mode: Mode::Full,
            ..SnapshotOptions::default()
        };
        let snap = snapshot(&t, &mut registry, &options, PageHeader::default()).unwrap();
        let body: Vec<&str> = snap.text.lines().skip(1).collect();
        assert_eq!(
            body,
            [
                "- textbox \"Password\" @1:e1 pwd",
                "- alert \"Wrong password\""
            ]
        );
    }

    #[test]
    fn long_names_are_truncated_and_quotes_escaped() {
        let long = "x".repeat(200);
        let t = tree(&[
            ("1", "", "RootWebArea", "", "", 1),
            ("2", "1", "link", &long, "", 2),
            ("3", "1", "button", "Say \"hi\"", "", 3),
        ]);
        let text = text_of(&t);
        let first = text.lines().next().unwrap();
        assert!(
            first.contains(&format!("\"{}…\"", "x".repeat(79))),
            "{first}"
        );
        assert!(text.contains(r#"button "Say \"hi\"" @1:e2"#), "{text}");
    }

    /// The root, then a chain of `depth` nested `generic` nodes, with a button inside the
    /// third one and a link at the very bottom.
    fn deep_chain(depth: usize) -> AxTree {
        let role = |r: &str| serde_json::json!({"type": "role", "value": r});
        let name = |n: &str| serde_json::json!({"type": "computedString", "value": n});
        let mut nodes = vec![serde_json::json!({
            "nodeId": "0", "role": role("RootWebArea"), "childIds": ["1"], "backendDOMNodeId": 1,
        })];
        for i in 1..=depth {
            let mut children = Vec::new();
            if i < depth {
                children.push((i + 1).to_string());
            } else {
                children.push("bottom".to_string());
            }
            if i == 3 {
                children.push("button".to_string());
            }
            nodes.push(serde_json::json!({
                "nodeId": i.to_string(), "parentId": (i - 1).to_string(), "role": role("generic"),
                "childIds": children, "backendDOMNodeId": i as i64 + 1,
            }));
        }
        nodes.push(serde_json::json!({
            "nodeId": "button", "parentId": "3", "role": role("button"), "name": name("Near the top"),
            "backendDOMNodeId": -1,
        }));
        nodes.push(serde_json::json!({
            "nodeId": "bottom", "parentId": depth.to_string(), "role": role("link"),
            "name": name("Deep down"), "backendDOMNodeId": -2,
        }));
        AxTree::from_cdp(&serde_json::json!({ "nodes": nodes })).unwrap()
    }

    #[test]
    fn very_deep_trees_are_cut_instead_of_overflowing_the_stack() {
        // Regression: the walk recursed once per level, so a page nesting 100k elements
        // overflowed the stack and aborted the whole app.
        let t = deep_chain(100_000);
        for mode in [Mode::Interactive, Mode::Full] {
            let mut registry = RefRegistry::new();
            let options = SnapshotOptions {
                mode,
                ..SnapshotOptions::default()
            };
            let snap = snapshot(&t, &mut registry, &options, PageHeader::default()).unwrap();
            let lines: Vec<&str> = snap.text.lines().collect();
            assert!(
                lines[1].starts_with("! the page tree is deeper than 512 levels"),
                "{}",
                snap.text
            );
            assert!(
                snap.text.contains("button \"Near the top\" @1:e1"),
                "{}",
                snap.text
            );
            assert!(!snap.text.contains("Deep down"), "{}", snap.text);
            assert!(snap.cut);
        }

        // find has the same limit.
        let mut registry = RefRegistry::new();
        let filter = crate::find::Filter {
            role: Some("link".into()),
            ..Default::default()
        };
        let found = crate::find::find(&t, &mut registry, &filter, 10).unwrap();
        assert_eq!(found.total, 0);
        assert!(found.too_deep);
        assert!(found.to_text().contains("deeper than 512 levels"));

        // A shallow tree has no notice.
        let shallow = deep_chain(20);
        let snap = snapshot(
            &shallow,
            &mut RefRegistry::new(),
            &SnapshotOptions::default(),
            PageHeader::default(),
        )
        .unwrap();
        assert!(!snap.cut && !snap.text.contains("! "), "{}", snap.text);
        assert!(snap.text.contains("link \"Deep down\""), "{}", snap.text);
    }

    #[test]
    fn registries_sharing_a_space_number_across_tabs() {
        let t = tree(&[
            ("1", "", "RootWebArea", "", "", 1),
            ("2", "1", "button", "OK", "", 2),
        ]);
        let space = crate::refs::RefSpace::new();
        let mut tab_a = RefRegistry::with_space(space.clone());
        let mut tab_b = RefRegistry::with_space(space);
        let options = SnapshotOptions::default();
        let a = snapshot(&t, &mut tab_a, &options, PageHeader::default()).unwrap();
        let b = snapshot(&t, &mut tab_b, &options, PageHeader::default()).unwrap();
        assert_eq!(a.refs[0].ref_str, "@1:e1");
        // Same page in another tab: the next snapshot id and a new e-number.
        assert_eq!(b.refs[0].ref_str, "@2:e2");
        assert!(tab_b.resolve("@1:e1").is_err());
        assert!(tab_b.resolve("e1").is_err());
        assert_eq!(tab_a.resolve("e1"), Ok(2));
    }
}
