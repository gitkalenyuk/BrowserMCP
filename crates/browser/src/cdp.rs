//! Small typed helpers over raw CDP calls (`ctx.backend.cdp(...)`), plus error mapping.
//!
//! Stealth rule: we never call `Runtime.enable` (pages can detect it). `Runtime.evaluate`
//! and `Runtime.callFunctionOn` work without it.
//!
//! Helpers that deal with DOM nodes or page objects take a `session`: `None` for the page
//! itself, `Some(id)` for the child session of an out-of-process iframe (see
//! [`crate::frames`]). Node ids, object ids and coordinates only mean something in the
//! session they came from.

use bmcp_core::{CmdError, Ctx, ErrorCode, TabId};
use bmcp_perception::PerceptionError;
use serde_json::{Value, json};

/// One raw CDP call; backend failures become `CmdError`s (`TAB_NOT_FOUND`, `TIMEOUT`, …).
pub async fn call(ctx: &Ctx, tab: TabId, method: &str, params: Value) -> Result<Value, CmdError> {
    Ok(ctx.backend.cdp(tab, method.to_string(), params).await?)
}

/// [`call`] in a session (`None` = the page).
pub async fn call_in(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, CmdError> {
    let session = session.map(str::to_string);
    Ok(ctx
        .backend
        .cdp_session(tab, session, method.to_string(), params)
        .await?)
}

/// Perception errors (bad/stale ref, bad filter, …) → command errors with advice.
pub fn perception_error(err: PerceptionError) -> CmdError {
    let message = err.to_string();
    match err {
        PerceptionError::StaleRef { .. } => CmdError::new(ErrorCode::StaleRef, message)
            .with_hint("Take a new page_snapshot (or page_find) and use the refs from it."),
        PerceptionError::BadRef { .. } => CmdError::new(ErrorCode::NotFound, message)
            .with_hint("Refs look like @4:e3 and come from page_snapshot or page_find."),
        PerceptionError::NotFound(_) => CmdError::new(ErrorCode::NotFound, message),
        PerceptionError::BadFilter(_) => CmdError::invalid_params(message),
        PerceptionError::BadTree(_) => CmdError::internal(message),
    }
}

/// `STALE_REF` for a ref whose DOM node is gone (removed by the page).
pub fn node_gone(what: &str) -> CmdError {
    CmdError::new(
        ErrorCode::StaleRef,
        format!("{what} is no longer in the page (it was removed or replaced)"),
    )
    .with_hint("Take a new page_snapshot and use the new ref.")
}

/// Main-frame `loaderId`: changes whenever the tab loads a new document.
pub async fn main_loader_id(ctx: &Ctx, tab: TabId) -> Result<String, CmdError> {
    let tree = call(ctx, tab, "Page.getFrameTree", json!({})).await?;
    Ok(tree["frameTree"]["frame"]["loaderId"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// `DOM.resolveNode`: a JavaScript object handle (`objectId`) for a DOM node.
/// Fails with `STALE_REF` when the node does not exist any more.
pub async fn resolve_object(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    backend_node_id: i64,
    what: &str,
) -> Result<String, CmdError> {
    let reply = ctx
        .backend
        .cdp_session(
            tab,
            session.map(str::to_string),
            "DOM.resolveNode".into(),
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
    match reply {
        Ok(reply) => reply["object"]["objectId"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| node_gone(what)),
        // "No node with given id found" and friends.
        Err(bmcp_core::BackendError::Cdp(_)) => Err(node_gone(what)),
        Err(other) => Err(other.into()),
    }
}

/// A `Runtime.callFunctionOn` argument holding a plain JSON value.
pub fn arg_value(value: Value) -> Value {
    json!({ "value": value })
}

/// A `Runtime.callFunctionOn` argument referring to a page object (by `objectId`).
pub fn arg_object(object_id: &str) -> Value {
    json!({ "objectId": object_id })
}

/// Run `function` (JavaScript source like `function(a) { return this.value + a }`) with `this`
/// bound to the page object `object_id`. Returns CDP's `RemoteObject` for the result: with
/// `by_value` its `value` field holds the JSON result, otherwise its `objectId` is a handle
/// to the returned object.
pub async fn call_on(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    object_id: &str,
    function: &str,
    arguments: Vec<Value>,
    by_value: bool,
) -> Result<Value, CmdError> {
    let reply = call_in(
        ctx,
        tab,
        session,
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": function,
            "arguments": arguments,
            "returnByValue": by_value,
            "awaitPromise": true,
        }),
    )
    .await?;
    check_exception(&reply)?;
    Ok(reply["result"].clone())
}

/// [`call_on`] with plain JSON arguments, returning the JSON result.
pub async fn call_function(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    object_id: &str,
    function: &str,
    args: &[Value],
) -> Result<Value, CmdError> {
    let arguments = args.iter().cloned().map(arg_value).collect();
    let result = call_on(ctx, tab, session, object_id, function, arguments, true).await?;
    Ok(result["value"].clone())
}

/// [`call_on`] returning a handle to the returned object (`None` for `null`/`undefined`).
pub async fn call_function_object(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    object_id: &str,
    function: &str,
) -> Result<Option<String>, CmdError> {
    let result = call_on(ctx, tab, session, object_id, function, Vec::new(), false).await?;
    Ok(result["objectId"].as_str().map(str::to_string))
}

/// `Runtime.evaluate` in the page, returning the value.
pub async fn evaluate(ctx: &Ctx, tab: TabId, expression: &str) -> Result<Value, CmdError> {
    let reply = call(
        ctx,
        tab,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
    )
    .await?;
    check_exception(&reply)?;
    Ok(reply["result"]["value"].clone())
}

/// `Runtime.evaluate` returning an object handle (e.g. `document.scrollingElement`).
pub async fn evaluate_object(
    ctx: &Ctx,
    tab: TabId,
    expression: &str,
) -> Result<Option<String>, CmdError> {
    let reply = call(
        ctx,
        tab,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": false }),
    )
    .await?;
    check_exception(&reply)?;
    Ok(reply["result"]["objectId"].as_str().map(str::to_string))
}

/// A thrown JavaScript exception → `INTERNAL` error with the exception text.
fn check_exception(reply: &Value) -> Result<(), CmdError> {
    let Some(details) = reply.get("exceptionDetails") else {
        return Ok(());
    };
    let text = details["exception"]["description"]
        .as_str()
        .or_else(|| details["text"].as_str())
        .unwrap_or("JavaScript exception");
    Err(CmdError::internal(format!(
        "script in the page failed: {text}"
    )))
}

/// A rectangle in CSS pixels, relative to the top-left of the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BoxRect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// The first visible box of a node (`DOM.getContentQuads`), or `None` when it has no layout
/// box (display:none, detached, zero size). In the viewport of the node's session.
pub async fn content_box(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    backend_node_id: i64,
) -> Result<Option<BoxRect>, CmdError> {
    let reply = ctx
        .backend
        .cdp_session(
            tab,
            session.map(str::to_string),
            "DOM.getContentQuads".into(),
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
    let reply = match reply {
        Ok(reply) => reply,
        // "Could not compute content quads." = not rendered.
        Err(bmcp_core::BackendError::Cdp(_)) => return Ok(None),
        Err(other) => return Err(other.into()),
    };
    let quads = reply["quads"].as_array().cloned().unwrap_or_default();
    // Each quad is 8 numbers: x1,y1 .. x4,y4 (corners, clockwise). Take its bounding box.
    for quad in quads {
        let points: Vec<f64> = quad
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if points.len() != 8 {
            continue;
        }
        let xs = [points[0], points[2], points[4], points[6]];
        let ys = [points[1], points[3], points[5], points[7]];
        let (min_x, max_x) = min_max(&xs);
        let (min_y, max_y) = min_max(&ys);
        let rect = BoxRect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        };
        if rect.width >= 1.0 && rect.height >= 1.0 {
            return Ok(Some(rect));
        }
    }
    Ok(None)
}

fn min_max(values: &[f64; 4]) -> (f64, f64) {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (min, max)
}

/// Layout facts from `Page.getLayoutMetrics` (all CSS pixels).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Layout {
    pub viewport_width: f64,
    pub viewport_height: f64,
    /// Scroll offset of the viewport inside the document.
    pub page_x: f64,
    pub page_y: f64,
    pub content_width: f64,
    pub content_height: f64,
}

/// Layout of the page (`session = None`) or of an out-of-process frame's own viewport.
pub async fn layout(ctx: &Ctx, tab: TabId, session: Option<&str>) -> Result<Layout, CmdError> {
    let m = call_in(ctx, tab, session, "Page.getLayoutMetrics", json!({})).await?;
    let num = |v: &Value| v.as_f64().unwrap_or(0.0);
    let viewport = &m["cssLayoutViewport"];
    let content = &m["cssContentSize"];
    Ok(Layout {
        viewport_width: num(&viewport["clientWidth"]),
        viewport_height: num(&viewport["clientHeight"]),
        page_x: num(&viewport["pageX"]),
        page_y: num(&viewport["pageY"]),
        content_width: num(&content["width"]),
        content_height: num(&content["height"]),
    })
}
