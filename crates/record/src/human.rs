//! Human actions: the recorder script (`recorder.js`) in an isolated world, and turning its
//! JSON events into [`Step`]s.
//!
//! Installation (no `Runtime.enable`, which pages can detect):
//!
//! 1. `Page.addScriptToEvaluateOnNewDocument {source, worldName: "bmcp-recorder"}` — for every
//!    later document;
//! 2. `Page.createIsolatedWorld {frameId, worldName}` — the context id of that world in the
//!    current document (the same name gives the same world back);
//! 3. `Runtime.addBinding {name: "__bmcpRecord", executionContextId}` — a function in *only*
//!    that context (the page cannot see it) that raises `Runtime.bindingCalled`;
//! 4. `Runtime.evaluate {contextId}` — run the recorder in the current document.
//!
//! Measured on CEF 156 (notes in `docs/spikes/S6-video.md`): `Runtime.bindingCalled` *does*
//! arrive without `Runtime.enable`, but a binding added by `executionContextName` is never
//! installed into isolated worlds while the Runtime domain is disabled; one added by
//! `executionContextId` is. So after every main-frame navigation the session adds the binding
//! to the new document's world again ([`bind`]) and drains what the script queued before that.
//!
//! The script says `hello` when it starts. If no `bindingCalled` arrives within 1.5 s, the
//! session switches the script to queue mode and drains the queue with `Runtime.evaluate`
//! every 250 ms instead (see `session.rs`).

use std::sync::Arc;

use bmcp_core::{BackendResult, BrowserBackend, TabId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{Source, Step, Target};

/// Name of the isolated world the recorder runs in.
pub const WORLD: &str = "bmcp-recorder";
/// Name of the CDP binding the recorder calls.
pub const BINDING: &str = "__bmcpRecord";

const RECORDER_JS: &str = include_str!("recorder.js");

/// How events get from the page to us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `Runtime.bindingCalled` events.
    Binding,
    /// Poll the in-page queue.
    Queue,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Binding => "binding",
            Mode::Queue => "queue",
        }
    }
}

/// The recorder source for `mode`.
pub fn source(mode: Mode) -> String {
    RECORDER_JS.replace("__BMCP_MODE__", mode.as_str())
}

/// One raw CDP call.
async fn cdp(
    backend: &Arc<dyn BrowserBackend>,
    tab: TabId,
    method: &str,
    params: Value,
) -> BackendResult<Value> {
    backend.cdp(tab, method.to_string(), params).await
}

/// Install the recorder for new documents and run it in the current one.
/// Returns the `addScriptToEvaluateOnNewDocument` identifier (to remove it on stop).
pub async fn install(
    backend: &Arc<dyn BrowserBackend>,
    tab: TabId,
    mode: Mode,
) -> BackendResult<String> {
    let script = source(mode);
    let reply = cdp(
        backend,
        tab,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": script, "worldName": WORLD }),
    )
    .await?;
    let identifier = reply["identifier"].as_str().unwrap_or_default().to_string();
    let context = match mode {
        Mode::Binding => bind(backend, tab).await?,
        Mode::Queue => world_context(backend, tab).await?,
    };
    evaluate_in(backend, tab, context, &script).await?;
    Ok(identifier)
}

/// Add the binding to the recorder's world in the current document; returns its context id.
pub async fn bind(backend: &Arc<dyn BrowserBackend>, tab: TabId) -> BackendResult<i64> {
    let context = world_context(backend, tab).await?;
    cdp(
        backend,
        tab,
        "Runtime.addBinding",
        json!({ "name": BINDING, "executionContextId": context }),
    )
    .await?;
    Ok(context)
}

/// The execution context id of our isolated world in the main frame (created if needed;
/// Chromium reuses the world of the same name, so this finds the recorder's context).
pub async fn world_context(backend: &Arc<dyn BrowserBackend>, tab: TabId) -> BackendResult<i64> {
    let tree = cdp(backend, tab, "Page.getFrameTree", json!({})).await?;
    let frame = tree["frameTree"]["frame"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let world = cdp(
        backend,
        tab,
        "Page.createIsolatedWorld",
        json!({ "frameId": frame, "worldName": WORLD }),
    )
    .await?;
    world["executionContextId"]
        .as_i64()
        .ok_or_else(|| bmcp_core::BackendError::Cdp("createIsolatedWorld gave no context".into()))
}

/// `Runtime.evaluate` in a context, returning the value.
pub async fn evaluate_in(
    backend: &Arc<dyn BrowserBackend>,
    tab: TabId,
    context: i64,
    expression: &str,
) -> BackendResult<Value> {
    let reply = cdp(
        backend,
        tab,
        "Runtime.evaluate",
        json!({ "expression": expression, "contextId": context, "returnByValue": true }),
    )
    .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("exception");
        return Err(bmcp_core::BackendError::Cdp(format!(
            "recorder script: {text}"
        )));
    }
    Ok(reply["result"]["value"].clone())
}

/// Read and clear the in-page queue (queue mode). Each item is one event's JSON text.
pub async fn drain(
    backend: &Arc<dyn BrowserBackend>,
    tab: TabId,
    context: i64,
) -> BackendResult<Vec<String>> {
    let value = evaluate_in(
        backend,
        tab,
        context,
        "globalThis.__bmcpRecorder ? globalThis.__bmcpRecorder.drain() : []",
    )
    .await?;
    Ok(value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// Remove the recorder: new documents stop getting it, the current one's listeners go.
/// Errors are ignored (the tab may be gone).
pub async fn uninstall(backend: &Arc<dyn BrowserBackend>, tab: TabId, script_id: &str) {
    if !script_id.is_empty() {
        let _ = cdp(
            backend,
            tab,
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({ "identifier": script_id }),
        )
        .await;
    }
    let _ = cdp(
        backend,
        tab,
        "Runtime.removeBinding",
        json!({ "name": BINDING }),
    )
    .await;
}

/// Tell the in-page recorder to stop (after a final flush of half-typed fields).
pub async fn stop_in_page(backend: &Arc<dyn BrowserBackend>, tab: TabId, context: i64) {
    let _ = evaluate_in(
        backend,
        tab,
        context,
        "globalThis.__bmcpRecorder && globalThis.__bmcpRecorder.stop()",
    )
    .await;
}

/// The element as the recorder script describes it.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DomTarget {
    role: Option<String>,
    name: Option<String>,
    text: Option<String>,
    css: Option<String>,
    tag: Option<String>,
    input_type: Option<String>,
    nth: Option<usize>,
}

/// One event from the recorder script.
#[derive(Debug, Default, Deserialize)]
struct DomEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    t: u64,
    url: Option<String>,
    target: Option<DomTarget>,
    value: Option<String>,
    #[serde(default)]
    secret: bool,
    key: Option<String>,
    label: Option<String>,
    x: Option<i64>,
    y: Option<i64>,
}

/// What the session does with one recorder event.
#[derive(Debug, PartialEq)]
pub enum HumanEvent {
    /// The script started in a document (proves the binding works).
    Hello,
    /// A step to log (its `ts_ms` is the page time of the action).
    Step(Box<Step>),
}

/// Parse one recorder event. `None` for garbage or unknown types.
pub fn parse_event(payload: &str) -> Option<HumanEvent> {
    let ev: DomEvent = serde_json::from_str(payload).ok()?;
    let target = ev.target.map(|t| Target {
        role: t.role.filter(|s| !s.is_empty()),
        name: t.name.filter(|s| !s.is_empty()),
        text: t.text.filter(|s| !s.is_empty()),
        nth: t.nth.filter(|n| *n > 0),
        css: t.css.filter(|s| !s.is_empty()),
        tag: t.tag,
        input_type: t.input_type,
        from: Some("dom".into()),
        ..Target::default()
    });
    let mut step = Step {
        ts_ms: ev.t,
        source: Source::Human,
        url: ev.url,
        target,
        ..Step::default()
    };
    match ev.kind.as_str() {
        "hello" => return Some(HumanEvent::Hello),
        "click" | "dblclick" | "check" | "uncheck" => step.action = ev.kind,
        "fill" => {
            step.action = "fill".into();
            step.secret = ev.secret;
            // Never trust the page side alone with secrets.
            step.value = if ev.secret {
                Some("***".into())
            } else {
                ev.value
            };
        }
        "select" => {
            step.action = "select".into();
            step.value = ev.value;
            step.extra = ev.label.map(|l| json!({ "label": l }));
        }
        "key" => {
            step.action = "press".into();
            step.value = ev.key;
        }
        "scroll" => {
            step.action = "scroll".into();
            step.value = Some("to".into());
            step.extra = Some(json!({ "x": ev.x.unwrap_or(0), "y": ev.y.unwrap_or(0) }));
        }
        _ => return None,
    }
    Some(HumanEvent::Step(Box::new(step)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_is_baked_into_the_source() {
        assert!(source(Mode::Queue).contains("let mode = \"queue\""));
        assert!(source(Mode::Binding).contains("let mode = \"binding\""));
        assert!(!source(Mode::Binding).contains("__BMCP_MODE__"));
    }

    #[test]
    fn parses_click_fill_key_and_hello() {
        assert_eq!(
            parse_event(r#"{"type":"hello","t":1}"#),
            Some(HumanEvent::Hello)
        );
        let Some(HumanEvent::Step(click)) = parse_event(
            r##"{"type":"click","t":5,"url":"http://x/","target":{"role":"button","name":"Sign in","text":"Sign in","css":"#login > p > button","tag":"button"}}"##,
        ) else {
            panic!("click not parsed");
        };
        assert_eq!(click.action, "click");
        assert_eq!(click.ts_ms, 5);
        let t = click.target.unwrap();
        assert_eq!(
            (t.role.as_deref(), t.name.as_deref()),
            (Some("button"), Some("Sign in"))
        );
        assert_eq!(t.from.as_deref(), Some("dom"));

        // A password: even if the page sent the value, we keep "***".
        let Some(HumanEvent::Step(fill)) =
            parse_event(r#"{"type":"fill","t":6,"value":"hunter2","secret":true}"#)
        else {
            panic!("fill not parsed");
        };
        assert_eq!(fill.value.as_deref(), Some("***"));
        assert!(fill.secret);

        let Some(HumanEvent::Step(key)) =
            parse_event(r#"{"type":"key","t":7,"key":"Enter","target":null}"#)
        else {
            panic!("key not parsed");
        };
        assert_eq!(
            (key.action.as_str(), key.value.as_deref()),
            ("press", Some("Enter"))
        );
        assert!(key.target.is_none());

        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"type":"mystery"}"#), None);
    }
}
