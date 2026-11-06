//! Serde model of the CDP accessibility tree (`Accessibility.getFullAXTree`), plus an
//! indexed [`AxTree`] that makes parent/child lookups cheap.
//!
//! Parsing is lenient on purpose: Chrome adds fields over time (`chromeRole`,
//! `ignoredReasons`, `sources`, ...). Unknown fields are ignored and missing fields fall
//! back to defaults, so a newer Chrome never breaks the parser.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{PerceptionError, Result};

/// A CDP `AXValue`: `{"type": "computedString", "value": "Sign in"}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AxValue {
    /// CDP value type, e.g. `"computedString"`, `"boolean"`, `"tristate"`, `"integer"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The raw JSON value (string, bool, number, ...). `None` when absent.
    pub value: Option<Value>,
}

impl AxValue {
    /// The value as text: strings as-is, numbers and booleans formatted, others empty.
    pub fn as_text(&self) -> String {
        match &self.value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }

    /// True for `true` and for the strings `"true"` (CDP uses both styles).
    pub fn is_true(&self) -> bool {
        match &self.value {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s == "true",
            _ => false,
        }
    }
}

/// One entry of a node's `properties` list: `{"name": "required", "value": {...}}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AxProperty {
    pub name: String,
    pub value: AxValue,
}

/// One node of `Accessibility.getFullAXTree`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AxNode {
    /// AX node id (a string in CDP). Unique within one tree.
    pub node_id: String,
    /// Chrome decided this node is not interesting (wrappers like `<div>`, `<body>`).
    /// Its children can still be interesting, so we walk through it.
    pub ignored: bool,
    pub role: Option<AxValue>,
    pub name: Option<AxValue>,
    pub description: Option<AxValue>,
    pub value: Option<AxValue>,
    pub properties: Vec<AxProperty>,
    pub child_ids: Vec<String>,
    pub parent_id: Option<String>,
    /// The DOM node behind this AX node. This is what actions (click, type) target,
    /// and what refs are keyed by.
    #[serde(rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    pub frame_id: Option<String>,
}

/// Borrow the string inside an optional `AxValue`, or `""`.
fn value_str(v: &Option<AxValue>) -> &str {
    v.as_ref()
        .and_then(|v| v.value.as_ref())
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

impl AxNode {
    /// Role as CDP reports it (`"button"`, `"StaticText"`, `"RootWebArea"`...), or `""`.
    pub fn role(&self) -> &str {
        value_str(&self.role)
    }

    /// Accessible name, or `""`.
    pub fn name(&self) -> &str {
        value_str(&self.name)
    }

    /// Accessible description, or `""`.
    pub fn description(&self) -> &str {
        value_str(&self.description)
    }

    /// Current value (text in a textbox, slider position...) as text, or `""`.
    pub fn value_text(&self) -> String {
        self.value.as_ref().map(|v| v.as_text()).unwrap_or_default()
    }

    /// Look up a property such as `"focusable"` or `"checked"`.
    pub fn prop(&self, name: &str) -> Option<&AxValue> {
        self.properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.value)
    }

    /// True when the property exists and is true.
    pub fn prop_true(&self, name: &str) -> bool {
        self.prop(name).is_some_and(|v| v.is_true())
    }

    /// Property as text (`""` when missing).
    pub fn prop_text(&self, name: &str) -> String {
        self.prop(name).map(|v| v.as_text()).unwrap_or_default()
    }
}

/// A screen rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Optional extra facts about a DOM node that the AX tree does not carry.
/// The caller builds these from `DOMSnapshot.captureSnapshot` and attaches them with
/// [`AxTree::with_layout`]. Every field is optional; missing info changes nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeLayout {
    /// Bounding box (reserved for future use: hit-testing, screenshot crops).
    pub bounds: Option<Rect>,
    /// `Some(false)` marks interactive nodes as `offscreen` in the snapshot.
    pub in_viewport: Option<bool>,
    /// DOM hint: the `<input type=...>` attribute. `"password"` forces `pwd` handling
    /// even when the accessible name does not say "password".
    pub input_type: Option<String>,
}

/// Indexed accessibility tree: the CDP nodes plus lookup tables.
#[derive(Debug, Clone, Default)]
pub struct AxTree {
    /// Nodes in the order CDP returned them (document order for Chrome).
    pub nodes: Vec<AxNode>,
    /// `nodeId` -> position in `nodes`.
    by_id: HashMap<String, usize>,
    /// `backendDOMNodeId` -> position in `nodes`.
    by_backend: HashMap<i64, usize>,
    /// Optional per-DOM-node extras, keyed by `backendDOMNodeId`.
    layout: HashMap<i64, NodeLayout>,
}

/// Shape of the CDP result object: `{"nodes": [...]}`.
#[derive(Deserialize)]
struct FullTreeResult {
    #[serde(default)]
    nodes: Vec<AxNode>,
}

impl AxTree {
    /// Build from CDP JSON. Accepts the result object `{"nodes": [...]}`, the whole
    /// response `{"id": 1, "result": {"nodes": [...]}}`, or a bare array of nodes.
    pub fn from_cdp(json: &Value) -> Result<AxTree> {
        // Unwrap `{"result": ...}` if the caller passed the whole CDP response.
        let json = json.get("result").unwrap_or(json);
        let nodes: Vec<AxNode> = if json.is_array() {
            serde_json::from_value(json.clone())
                .map_err(|e| PerceptionError::BadTree(e.to_string()))?
        } else if json.get("nodes").is_some() {
            let parsed: FullTreeResult = serde_json::from_value(json.clone())
                .map_err(|e| PerceptionError::BadTree(e.to_string()))?;
            parsed.nodes
        } else {
            return Err(PerceptionError::BadTree(
                "expected {\"nodes\": [...]} from Accessibility.getFullAXTree".to_string(),
            ));
        };
        Ok(AxTree::from_nodes(nodes))
    }

    /// Parse a JSON string (handy in tests).
    pub fn from_json_str(text: &str) -> Result<AxTree> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| PerceptionError::BadTree(e.to_string()))?;
        AxTree::from_cdp(&value)
    }

    /// Build the lookup tables for an already-parsed node list.
    pub fn from_nodes(nodes: Vec<AxNode>) -> AxTree {
        let mut by_id = HashMap::new();
        let mut by_backend: HashMap<i64, usize> = HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            by_id.insert(node.node_id.clone(), index);
            if let Some(backend) = node.backend_dom_node_id {
                // Several AX nodes can share one DOM node; prefer the non-ignored one.
                let replace = match by_backend.get(&backend) {
                    None => true,
                    Some(&old) => nodes[old].ignored && !node.ignored,
                };
                if replace {
                    by_backend.insert(backend, index);
                }
            }
        }
        AxTree {
            nodes,
            by_id,
            by_backend,
            layout: HashMap::new(),
        }
    }

    /// Attach optional per-node layout/DOM info (see [`NodeLayout`]).
    pub fn with_layout(mut self, layout: HashMap<i64, NodeLayout>) -> AxTree {
        self.layout = layout;
        self
    }

    /// Layout info for a DOM node, if the caller provided any.
    pub fn layout(&self, backend_node_id: i64) -> Option<&NodeLayout> {
        self.layout.get(&backend_node_id)
    }

    /// The root node: the first node without a (known) parent, usually `RootWebArea`.
    pub fn root(&self) -> Option<usize> {
        self.nodes.iter().position(|n| match &n.parent_id {
            None => true,
            Some(parent) => !self.by_id.contains_key(parent),
        })
    }

    /// Positions of a node's children (unknown child ids are skipped).
    pub fn children(&self, index: usize) -> Vec<usize> {
        self.nodes[index]
            .child_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).copied())
            .collect()
    }

    /// Position of a node's parent.
    pub fn parent(&self, index: usize) -> Option<usize> {
        let parent_id = self.nodes[index].parent_id.as_ref()?;
        self.by_id.get(parent_id).copied()
    }

    /// Find the node for a `backendDOMNodeId`.
    pub fn index_of_backend(&self, backend_node_id: i64) -> Option<usize> {
        self.by_backend.get(&backend_node_id).copied()
    }

    /// Find the node for an AX `nodeId`.
    pub fn index_of_node_id(&self, node_id: &str) -> Option<usize> {
        self.by_id.get(node_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_leniently() {
        // Unknown fields (chromeRole), missing fields (name, properties) and the full
        // CDP response wrapper are all fine.
        let json = r#"{"id": 5, "result": {"nodes": [
            {"nodeId": "1", "role": {"type": "internalRole", "value": "RootWebArea"},
             "chromeRole": {"type": "internalRole", "value": 144}, "childIds": ["2"]},
            {"nodeId": "2", "parentId": "1", "role": {"type": "role", "value": "button"},
             "name": {"type": "computedString", "value": "OK"},
             "properties": [{"name": "focusable", "value": {"type": "booleanOrUndefined", "value": true}}],
             "backendDOMNodeId": 7}
        ]}}"#;
        let tree = AxTree::from_json_str(json).unwrap();
        assert_eq!(tree.nodes.len(), 2);
        assert_eq!(tree.root(), Some(0));
        assert_eq!(tree.children(0), vec![1]);
        assert_eq!(tree.parent(1), Some(0));
        let button = &tree.nodes[1];
        assert_eq!(button.role(), "button");
        assert_eq!(button.name(), "OK");
        assert!(button.prop_true("focusable"));
        assert_eq!(tree.index_of_backend(7), Some(1));
    }

    #[test]
    fn rejects_non_tree_json() {
        let err = AxTree::from_json_str(r#"{"hello": 1}"#).unwrap_err();
        assert_eq!(err.code(), "BAD_TREE");
    }
}
