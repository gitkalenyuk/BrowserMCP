//! Iframes: one accessibility tree for the page and all its frames, and where each node lives.
//!
//! * **Same-process frames** (same site as the page) share the page's CDP session and DOM node
//!   numbering: their tree comes from `Accessibility.getFullAXTree {frameId}` and their nodes
//!   are used like the page's own.
//! * **Out-of-process frames** (OOPIFs, another site) are separate targets. With
//!   `Target.setAutoAttach {flatten: true}` Chromium attaches each one as a child session
//!   (`Target.attachedToTarget`, whose `targetId` is the frame id). Their DOM nodes are
//!   numbered on their own, so in the merged tree a node gets the id `slot << 32 | backendId`,
//!   where the slot names the frame ([`Frames::locate`] turns it back).
//!
//! Coordinates: boxes and hit tests in a child session are in that frame's viewport; mouse
//! input goes to the page at top-level coordinates, so [`frame_offset`] adds the position of
//! each `<iframe>`'s content box in its parent.

use std::collections::HashMap;

use bmcp_core::{CdpEvents, CmdError, Ctx, TabId};
use bmcp_perception::AxTree;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::TryRecvError;

use crate::cdp::{self, perception_error};

/// At most this many frames are merged into one snapshot.
pub const MAX_FRAMES: usize = 32;
/// Frames nested deeper than this (iframe in iframe in ...) are not merged.
const MAX_FRAME_DEPTH: usize = 6;
/// Bits of a merged node id that hold the DOM node id; the bits above hold the frame slot.
const SLOT_SHIFT: u32 = 32;

/// One out-of-process frame we have seen in this tab.
#[derive(Debug, Clone)]
struct Slot {
    frame_id: String,
    /// Its child session.
    session: String,
    /// Slot of the document that contains its `<iframe>` (0 = the page).
    parent: usize,
    /// DOM node id of that `<iframe>` element, in the parent's numbering.
    owner: i64,
}

/// Where a DOM node from the merged tree lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeAt {
    /// The node's id in its own session.
    pub backend: i64,
    /// The child session of its frame; `None` = the page itself.
    pub session: Option<String>,
    /// Its frame slot (0 = the page or a same-process frame).
    pub slot: usize,
}

/// Per-tab frame bookkeeping (part of [`crate::TabState`]).
#[derive(Debug, Default)]
pub struct Frames {
    /// Slot `k >= 1` is `slots[k - 1]`; slot 0 is the page itself.
    slots: Vec<Slot>,
    /// Attached child sessions: frame (target) id → session id.
    sessions: HashMap<String, String>,
    /// Our subscription to the tab's CDP events (attach/detach of child sessions).
    events: Option<CdpEvents>,
    /// The backend has no events or no auto-attach: only same-process frames work.
    unsupported: bool,
}

impl Frames {
    /// Merged node id for DOM node `backend` of frame `slot`.
    pub fn merged_id(slot: usize, backend: i64) -> i64 {
        if slot == 0 {
            backend
        } else {
            ((slot as i64) << SLOT_SHIFT) | backend
        }
    }

    /// Turn a merged node id back into the frame and the node's own id.
    pub fn locate(&self, merged: i64) -> NodeAt {
        let slot = (merged >> SLOT_SHIFT) as usize;
        match self.slots.get(slot.wrapping_sub(1)) {
            Some(s) if slot > 0 => NodeAt {
                backend: merged & ((1 << SLOT_SHIFT) - 1),
                session: Some(s.session.clone()),
                slot,
            },
            _ => NodeAt {
                backend: merged,
                session: None,
                slot: 0,
            },
        }
    }

    /// The session of a slot (`None` for the page).
    pub fn session_of(&self, slot: usize) -> Option<String> {
        slot.checked_sub(1)
            .and_then(|i| self.slots.get(i))
            .map(|s| s.session.clone())
    }

    /// The `<iframe>` element in the page that (perhaps through more frames) holds `slot`.
    pub fn outermost_owner(&self, mut slot: usize) -> Option<i64> {
        let mut owner = None;
        while let Some(s) = slot.checked_sub(1).and_then(|i| self.slots.get(i)) {
            owner = Some(s.owner);
            slot = s.parent;
        }
        owner
    }

    /// The tab shows a new document: forget the frame slots (their refs are stale anyway).
    pub fn new_document(&mut self) {
        self.slots.clear();
    }

    /// The slot of an out-of-process frame, made on first sight. Slots stay the same for a
    /// frame (by frame id), so its refs stay the same across snapshots.
    fn slot_for(&mut self, frame_id: &str, session: &str, parent: usize, owner: i64) -> usize {
        if let Some(i) = self.slots.iter().position(|s| s.frame_id == frame_id) {
            let slot = &mut self.slots[i];
            slot.session = session.to_string();
            slot.parent = parent;
            slot.owner = owner;
            return i + 1;
        }
        self.slots.push(Slot {
            frame_id: frame_id.to_string(),
            session: session.to_string(),
            parent,
            owner,
        });
        self.slots.len()
    }
}

fn auto_attach(on: bool) -> Value {
    json!({ "autoAttach": on, "waitForDebuggerOnStart": false, "flatten": true })
}

/// Start following the tab's out-of-process frames (once), then apply the attach/detach
/// events that arrived since the last call. Failures only mean "no OOPIF support".
async fn sync_sessions(ctx: &Ctx, tab: TabId, frames: &mut Frames) {
    if frames.unsupported {
        return;
    }
    if frames.events.is_none() {
        // Subscribe first: Chromium announces the frames that exist already while it
        // handles setAutoAttach, before it answers.
        let Ok(events) = ctx.backend.cdp_events(tab).await else {
            frames.unsupported = true;
            return;
        };
        frames.events = Some(events);
        if cdp::call(ctx, tab, "Target.setAutoAttach", auto_attach(true))
            .await
            .is_err()
        {
            frames.unsupported = true;
            frames.events = None;
            return;
        }
    }
    if drain_events(ctx, tab, frames).await {
        // We missed events, so the session list may be wrong. Turning auto-attach off and on
        // detaches and re-announces every frame. (Missing events again would only leave
        // some out-of-process frame out of this snapshot.)
        frames.sessions.clear();
        let _ = cdp::call(ctx, tab, "Target.setAutoAttach", auto_attach(false)).await;
        let _ = cdp::call(ctx, tab, "Target.setAutoAttach", auto_attach(true)).await;
        drain_events(ctx, tab, frames).await;
    }
}

/// Apply queued attach/detach events. New child sessions get auto-attach too, so frames
/// inside them are announced (several rounds for deeper nesting). Returns true when the
/// receiver lagged (events were lost).
async fn drain_events(ctx: &Ctx, tab: TabId, frames: &mut Frames) -> bool {
    let mut lagged = false;
    for _round in 0..MAX_FRAME_DEPTH {
        let Some(events) = frames.events.as_mut() else {
            return lagged;
        };
        let mut new_sessions = Vec::new();
        loop {
            match events.try_recv() {
                Ok(event) => match event.method.as_str() {
                    "Target.attachedToTarget" => {
                        let info = &event.params["targetInfo"];
                        let session = event.params["sessionId"].as_str().unwrap_or_default();
                        let target = info["targetId"].as_str().unwrap_or_default();
                        if info["type"] == "iframe" && !session.is_empty() && !target.is_empty() {
                            frames
                                .sessions
                                .insert(target.to_string(), session.to_string());
                            new_sessions.push(session.to_string());
                        }
                    }
                    "Target.detachedFromTarget" => {
                        let session = event.params["sessionId"].as_str().unwrap_or_default();
                        frames.sessions.retain(|_, s| s != session);
                    }
                    _ => {}
                },
                Err(TryRecvError::Lagged(_)) => lagged = true,
                // Empty, or the tab closed.
                Err(_) => break,
            }
        }
        if new_sessions.is_empty() {
            break;
        }
        for session in new_sessions {
            let _ = ctx
                .backend
                .cdp_session(
                    tab,
                    Some(session),
                    "Target.setAutoAttach".into(),
                    auto_attach(true),
                )
                .await;
        }
    }
    lagged
}

/// `Accessibility.getFullAXTree` in a session, parsed.
async fn fetch_tree(
    ctx: &Ctx,
    tab: TabId,
    session: Option<String>,
    params: Value,
) -> Result<AxTree, CmdError> {
    let raw = ctx
        .backend
        .cdp_session(tab, session, "Accessibility.getFullAXTree".into(), params)
        .await?;
    AxTree::from_cdp(&raw).map_err(perception_error)
}

/// The accessibility tree of the page with every frame's tree attached under its `iframe`
/// node (up to [`MAX_FRAMES`]). A frame that cannot be read is left out, never an error.
pub async fn merged_tree(ctx: &Ctx, tab: TabId, frames: &mut Frames) -> Result<AxTree, CmdError> {
    let mut tree = fetch_tree(ctx, tab, None, json!({})).await?;
    sync_sessions(ctx, tab, frames).await;

    // (position of an iframe node, nesting depth). The frame that contains the iframe node
    // follows from the node's merged id.
    let mut todo: Vec<(usize, usize)> =
        iframe_nodes(&tree, 0).into_iter().map(|i| (i, 1)).collect();
    let mut merged = 0;
    // Frame id → URL, per session, fetched when first needed.
    let mut urls: HashMap<Option<String>, HashMap<String, String>> = HashMap::new();
    while let Some((index, depth)) = todo.pop() {
        if merged >= MAX_FRAMES || depth > MAX_FRAME_DEPTH {
            continue;
        }
        let Some(owner) = tree.nodes[index].backend_dom_node_id else {
            continue;
        };
        let owner = frames.locate(owner);
        // Which frame does this <iframe> show, and from where?
        let Ok(described) = ctx
            .backend
            .cdp_session(
                tab,
                owner.session.clone(),
                "DOM.describeNode".into(),
                json!({ "backendNodeId": owner.backend }),
            )
            .await
        else {
            continue;
        };
        let node = &described["node"];
        let Some(frame_id) = node["frameId"].as_str().map(str::to_string) else {
            continue;
        };
        // The frame's real (absolute) URL; the `src` attribute when the browser has none.
        let src = attribute(node, "src").unwrap_or_default();
        let session_urls = match urls.get(&owner.session) {
            Some(known) => known,
            None => {
                let known = frame_urls(ctx, tab, owner.session.clone()).await;
                urls.entry(owner.session.clone()).or_insert(known)
            }
        };
        let url = session_urls.get(&frame_id).cloned().unwrap_or(src);
        if !tree.nodes[index].child_ids.is_empty() {
            // Chromium already put the frame's document in the tree (same process).
            tree.nodes[index].frame_url = Some(url);
            continue;
        }
        let (child, child_slot) = match frames.sessions.get(&frame_id).cloned() {
            // Out of process: its own session and its own DOM node numbering.
            Some(session) => {
                let Ok(child) = fetch_tree(ctx, tab, Some(session.clone()), json!({})).await else {
                    continue;
                };
                let child_slot = frames.slot_for(&frame_id, &session, owner.slot, owner.backend);
                (child, child_slot)
            }
            // Same process as its parent: same session and numbering.
            None => {
                let params = json!({ "frameId": frame_id });
                let Ok(child) = fetch_tree(ctx, tab, owner.session.clone(), params).await else {
                    continue;
                };
                (child, owner.slot)
            }
        };
        let first_new = tree.nodes.len();
        let prefix = format!("{frame_id}:");
        tree.attach_frame(index, child, &prefix, &url, |b| {
            Frames::merged_id(child_slot, b)
        });
        merged += 1;
        // Iframes inside the frame just attached.
        for i in iframe_nodes(&tree, first_new) {
            todo.push((i, depth + 1));
        }
    }
    Ok(tree)
}

/// Frame id → URL of every frame a session knows (`Page.getFrameTree`; same-process child
/// frames included). Empty when the call fails.
async fn frame_urls(ctx: &Ctx, tab: TabId, session: Option<String>) -> HashMap<String, String> {
    fn collect(node: &Value, out: &mut HashMap<String, String>) {
        let frame = &node["frame"];
        if let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
            out.insert(id.to_string(), url.to_string());
        }
        for child in node["childFrames"].as_array().into_iter().flatten() {
            collect(child, out);
        }
    }
    let mut out = HashMap::new();
    if let Ok(reply) = ctx
        .backend
        .cdp_session(tab, session, "Page.getFrameTree".into(), json!({}))
        .await
    {
        collect(&reply["frameTree"], &mut out);
    }
    out
}

/// Positions of the iframe nodes at or after `from`.
fn iframe_nodes(tree: &AxTree, from: usize) -> Vec<usize> {
    (from..tree.nodes.len())
        .filter(|&i| tree.nodes[i].role() == "Iframe")
        .collect()
}

/// An attribute of a `DOM.Node` (`attributes` is a flat `[name, value, ...]` list).
fn attribute(node: &Value, name: &str) -> Option<String> {
    let list = node["attributes"].as_array()?;
    list.chunks(2)
        .find(|pair| pair[0] == name)
        .and_then(|pair| pair.get(1))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Where the viewport of frame `slot` sits in the page's viewport (CSS pixels): the sum of
/// the content-box corners of the `<iframe>` elements around it. `(0, 0)` for the page.
pub async fn frame_offset(
    ctx: &Ctx,
    tab: TabId,
    frames: &Frames,
    mut slot: usize,
) -> Result<(f64, f64), CmdError> {
    let (mut x, mut y) = (0.0, 0.0);
    while let Some(s) = slot.checked_sub(1).and_then(|i| frames.slots.get(i)) {
        let model = ctx
            .backend
            .cdp_session(
                tab,
                frames.session_of(s.parent),
                "DOM.getBoxModel".into(),
                json!({ "backendNodeId": s.owner }),
            )
            .await?;
        // The content quad starts with its top-left corner: x1, y1.
        let content = &model["model"]["content"];
        x += content[0].as_f64().unwrap_or(0.0);
        y += content[1].as_f64().unwrap_or(0.0);
        slot = s.parent;
    }
    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_ids_round_trip() {
        let mut frames = Frames::default();
        let slot = frames.slot_for("F1", "S1", 0, 30);
        assert_eq!(slot, 1);
        let id = Frames::merged_id(slot, 7);
        assert_eq!(
            frames.locate(id),
            NodeAt {
                backend: 7,
                session: Some("S1".into()),
                slot: 1
            }
        );
        // Page nodes are unchanged.
        assert_eq!(frames.locate(7).session, None);
        // Same frame again: same slot, new session.
        assert_eq!(frames.slot_for("F1", "S2", 0, 30), 1);
        assert_eq!(frames.session_of(1).as_deref(), Some("S2"));
        let nested = frames.slot_for("F2", "S3", 1, 4);
        assert_eq!(frames.outermost_owner(nested), Some(30));
        frames.new_document();
        assert_eq!(frames.locate(id).session, None);
    }

    #[test]
    fn reads_attributes() {
        let node = json!({ "attributes": ["id", "x", "src", "https://a.test/"] });
        assert_eq!(attribute(&node, "src").as_deref(), Some("https://a.test/"));
        assert_eq!(attribute(&node, "title"), None);
    }
}
