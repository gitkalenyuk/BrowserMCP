//! BrowserMCP perception: turn the Chrome accessibility tree into compact text an LLM
//! can read, with refs it can act on. No screenshots, no CEF dependency: input is the
//! `serde_json::Value` returned by CDP `Accessibility.getFullAXTree`.
//!
//! ```text
//! snap 7 tab=3 "Sign in – Acme" https://acme.com/login vp=1280x720 y=0/1840
//! - main
//!   - heading "Sign in" h1
//!   - textbox "Email" @7:e3 req
//!   - textbox "Password" @7:e4 pwd
//!   - button "Sign in" @7:e6 disabled
//!   - list "Providers" 12 items, 3 shown → @7:g1
//! ```
//!
//! Typical use, once per tab:
//!
//! ```
//! use bmcp_perception::{AxTree, PageHeader, RefRegistry, SnapshotOptions, snapshot};
//!
//! let cdp_result = serde_json::json!({"nodes": [
//!     {"nodeId": "1", "role": {"type": "internalRole", "value": "RootWebArea"},
//!      "name": {"type": "computedString", "value": "Demo"}, "childIds": ["2"]},
//!     {"nodeId": "2", "parentId": "1", "role": {"type": "role", "value": "button"},
//!      "name": {"type": "computedString", "value": "OK"}, "backendDOMNodeId": 42}
//! ]});
//! let tree = AxTree::from_cdp(&cdp_result).unwrap();
//! // Keep one per tab. With several tabs use `RefRegistry::with_space` and one shared
//! // `RefSpace`, so refs are unique across tabs.
//! let mut registry = RefRegistry::new();
//! let header = PageHeader { tab: 1, title: "Demo".into(), url: "https://demo.test/".into(), ..Default::default() };
//! let snap = snapshot(&tree, &mut registry, &SnapshotOptions::default(), header).unwrap();
//! assert_eq!(snap.text, "snap 1 tab=1 \"Demo\" https://demo.test/\n- button \"OK\" @1:e1");
//! assert_eq!(registry.resolve("e1"), Ok(42)); // backendDOMNodeId to click
//! ```
//!
//! Modules: [`ax`] (CDP model), [`refs`] (ref registry), [`snapshot`], [`find`], [`diff`].

pub mod ax;
pub mod diff;
pub mod error;
pub mod find;
pub mod refs;
mod roles;
pub mod snapshot;
mod text;

pub use ax::{AxNode, AxProperty, AxTree, AxValue, NodeLayout, Rect};
pub use diff::diff;
pub use error::{PerceptionError, Result};
pub use find::{Filter, FindResult, Match, find};
pub use refs::{
    GroupTarget, ParsedRef, RefKind, RefRegistry, RefSpace, parse_ref, resolve, resolve_group,
};
pub use snapshot::{
    MAX_TREE_DEPTH, Mode, NodeEntry, PageHeader, RefEntry, Scroll, Snapshot, SnapshotOptions,
    Viewport, snapshot,
};
