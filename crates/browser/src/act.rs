//! The engine behind `page.act`: resolve a ref, make sure the element can really be used
//! ("actionability"), then send trusted input the way a person would.
//!
//! For a click that means: ref → DOM node → scroll it into view → it must be visible, have a
//! box and be enabled → hit-test its centre (is something else on top, like a cookie dialog?)
//! → `Input.dispatchMouseEvent` moved/pressed/released. Failures come back as
//! `NOT_INTERACTABLE` with a hint that names what is in the way.
//!
//! Elements inside an out-of-process iframe are checked in the frame's CDP session; mouse
//! input still goes to the page, at coordinates moved by the frame's position
//! ([`crate::frames`]). Shadow DOM needs no special path: hit tests and checks walk up
//! through shadow roots to their hosts.

use std::time::Duration;

use bmcp_core::{CmdError, Ctx, ErrorCode, TabId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cdp::{self, BoxRect};
use crate::frames::{self, Frames};
use crate::human;
use crate::keys;
use crate::page;
use crate::state::TabState;

/// How long an element may take to become usable (animations, late enabling) before we give up.
pub const ACTIONABLE_WAIT: Duration = Duration::from_millis(1500);
const RETRY_PAUSE: Duration = Duration::from_millis(200);

/// What to do with the element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Left click in the middle of the element.
    Click,
    /// Double click.
    Dblclick,
    /// Replace the field's text with `value` (focus, select all, type, fire change).
    Fill,
    /// Type `value` at the cursor without clearing the field first.
    Type,
    /// Pick the `<select>` option whose value or label is `value`.
    Select,
    /// Make a checkbox/radio/switch checked (clicks only if needed).
    Check,
    /// Make a checkbox/switch unchecked (clicks only if needed).
    Uncheck,
    /// Move the mouse over the element.
    Hover,
    /// Focus the element (if a ref is given) and press the keys in `value`, e.g. "Enter".
    Press,
    /// Scroll: `value` = into_view (default), up, down, top, bottom, left, right. Without a ref
    /// it scrolls the page.
    Scroll,
    /// Give the element keyboard focus.
    Focus,
}

impl Action {
    fn past_tense(self) -> &'static str {
        match self {
            Action::Click => "clicked",
            Action::Dblclick => "double-clicked",
            Action::Fill => "filled",
            Action::Type => "typed into",
            Action::Select => "selected in",
            Action::Check => "checked",
            Action::Uncheck => "unchecked",
            Action::Hover => "hovered",
            Action::Press => "pressed keys on",
            Action::Scroll => "scrolled",
            Action::Focus => "focused",
        }
    }

    /// Actions that send mouse or keyboard input (which Chromium drops for hidden pages).
    fn sends_input(self) -> bool {
        !matches!(self, Action::Select | Action::Scroll | Action::Focus)
    }

    /// Actions that may start a navigation, so it is worth waiting for one afterwards.
    pub fn may_navigate(self) -> bool {
        matches!(
            self,
            Action::Click | Action::Dblclick | Action::Press | Action::Check | Action::Uncheck
        )
    }
}

/// One action to perform.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Element ref from page_snapshot / page_find, e.g. "@4:e3" (or bare "e3" = latest
    /// snapshot). Optional only for press (focused element) and scroll (the page).
    #[serde(rename = "ref")]
    pub target: Option<String>,
    pub action: Action,
    /// Text for fill/type, option for select, keys for press ("Enter", "Control+A",
    /// "Tab Tab"), direction for scroll.
    pub value: Option<String>,
}

/// What we learn about an element from the page (see [`PROBE_JS`]).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Probe {
    connected: bool,
    tag: String,
    #[serde(rename = "type")]
    input_type: String,
    disabled: bool,
    readonly: bool,
    hidden: bool,
    editable: bool,
    /// `checked` / `aria-checked` / `aria-pressed`, when the element has such a state.
    checked: Option<bool>,
}

/// Runs with `this` = the element. Returns facts as JSON (see [`Probe`]).
const PROBE_JS: &str = r#"function() {
  const el = this.nodeType === 1 ? this : this.parentElement;
  if (!el || !el.isConnected) return { connected: false };
  const tag = el.tagName.toLowerCase();
  const type = (el.getAttribute('type') || '').toLowerCase();
  let disabled = false;
  try { disabled = el.matches(':disabled'); } catch (e) {}
  disabled = disabled || !!el.closest('[aria-disabled="true"]');
  const style = getComputedStyle(el);
  const hidden = style.visibility === 'hidden' || style.visibility === 'collapse' || style.display === 'none';
  const notText = ['checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'image', 'hidden', 'range', 'color'];
  const editable = (tag === 'input' && !notText.includes(type)) || tag === 'textarea' || el.isContentEditable;
  const readonly = !!el.readOnly || el.getAttribute('aria-readonly') === 'true';
  let checked = null;
  if (tag === 'input' && (type === 'checkbox' || type === 'radio')) checked = el.checked;
  else if (el.hasAttribute('aria-checked')) checked = el.getAttribute('aria-checked') === 'true';
  else if (el.hasAttribute('aria-pressed')) checked = el.getAttribute('aria-pressed') === 'true';
  return { connected: true, tag, type, disabled, readonly, hidden, editable, checked };
}"#;

/// `this` = target node, argument = the node the hit test found. True when a click there
/// reaches the target: the hit node is the target, inside it (also across shadow roots), or
/// inside one of its `<label>`s.
///
/// A text node (a `page.find` text match) stands for its parent element: the hit test never
/// returns text nodes, it reports the element around them, which is where the click goes.
const HIT_OK_JS: &str = r#"function(hit) {
  const self = this.nodeType === 1 ? this : this.parentElement;
  if (!self) return false;
  // Is `n` the element `el` or inside it? Walks up through shadow roots to their hosts.
  const within = (n, el) => { for (; n; n = n.parentNode || n.host) { if (n === el) return true; } return false; };
  if (within(hit, self)) return true;
  if (self.labels) { for (const l of self.labels) { if (within(hit, l)) return true; } }
  return false;
}"#;

/// `this` = the node on top. Returns the element worth naming: the dialog it belongs to (also
/// when the node sits in the dialog's shadow DOM), or the dialog inside it (a dimmed backdrop
/// around a cookie banner), else the element itself.
const COVER_JS: &str = r#"function() {
  const el = this.nodeType === 1 ? this : this.parentElement;
  if (!el) return null;
  const dialog = 'dialog, [role="dialog"], [role="alertdialog"], [aria-modal="true"]';
  // Like el.closest(dialog), but continuing from a shadow root to its host.
  for (let n = el; n; ) {
    if (n.matches(dialog)) return n;
    const root = n.getRootNode();
    n = n.parentElement || (root instanceof ShadowRoot ? root.host : null);
  }
  return el.querySelector(dialog) || el;
}"#;

/// `this` = an element. A short CSS-like description: `div#cookie-banner` or `div.overlay.dark`.
const CSS_NAME_JS: &str = r#"function() {
  let s = this.tagName.toLowerCase();
  if (this.id) return s + '#' + this.id;
  if (this.classList && this.classList.length) s += '.' + [...this.classList].slice(0, 2).join('.');
  return s;
}"#;

/// `this` = a field. Select all its text so typing replaces it.
const SELECT_ALL_JS: &str = r#"function() {
  if ((this.tagName === 'INPUT' || this.tagName === 'TEXTAREA') && typeof this.select === 'function') {
    this.select();
  } else if (this.isContentEditable) {
    const range = document.createRange();
    range.selectNodeContents(this);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  }
}"#;

/// `this` = an element. Fire `change` like a person leaving the field would.
const CHANGE_JS: &str =
    r#"function() { this.dispatchEvent(new Event('change', { bubbles: true })); }"#;

/// `this` = an `<input>` whose value cannot be typed (date, color, range…). Set it directly
/// through the native setter (so frameworks notice) and fire input + change.
const SET_VALUE_JS: &str = r#"function(v) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setter.call(this, v);
  this.dispatchEvent(new Event('input', { bubbles: true }));
  this.dispatchEvent(new Event('change', { bubbles: true }));
  return this.value;
}"#;

/// Input types where typing does not work and the value is set directly.
const SET_BY_VALUE_TYPES: &[&str] = &[
    "date",
    "time",
    "datetime-local",
    "month",
    "week",
    "color",
    "range",
];

/// `this` = a `<select>`. Pick the option by value, then exact label, then loose label.
const SELECT_JS: &str = r#"function(wanted) {
  if (this.tagName !== 'SELECT') return { error: 'not_select' };
  const norm = s => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const opts = [...this.options];
  const o = opts.find(o => o.value === wanted)
    || opts.find(o => o.label === wanted || o.text === wanted)
    || opts.find(o => norm(o.label) === norm(wanted));
  if (!o) return { error: 'no_option', options: opts.slice(0, 30).map(o => o.label || o.value) };
  if (o.disabled) return { error: 'disabled_option' };
  this.value = o.value;
  o.selected = true;
  this.dispatchEvent(new Event('input', { bubbles: true }));
  this.dispatchEvent(new Event('change', { bubbles: true }));
  return { selected: o.label || o.value };
}"#;

/// `this` = an element (or the page's scrolling element). Scroll it, or the nearest scrollable
/// ancestor, in a direction. Returns the new scroll position.
const SCROLL_JS: &str = r#"function(dir) {
  const page = document.scrollingElement || document.documentElement;
  const scrollable = e => {
    if (!e || e === page || e === document.body) return false;
    const s = getComputedStyle(e);
    return (e.scrollHeight > e.clientHeight + 1 && /(auto|scroll)/.test(s.overflowY))
        || (e.scrollWidth > e.clientWidth + 1 && /(auto|scroll)/.test(s.overflowX));
  };
  let t = this.nodeType === 1 ? this : this.parentElement;
  while (t && !scrollable(t)) t = t.parentElement;
  if (!t) t = page;
  const h = t === page ? window.innerHeight : t.clientHeight;
  const w = t === page ? window.innerWidth : t.clientWidth;
  const by = (x, y) => t.scrollBy({ left: x, top: y, behavior: 'instant' });
  switch (dir) {
    case 'up': by(0, -0.8 * h); break;
    case 'down': by(0, 0.8 * h); break;
    case 'left': by(-0.8 * w, 0); break;
    case 'right': by(0.8 * w, 0); break;
    case 'top': t.scrollTo({ top: 0, behavior: 'instant' }); break;
    case 'bottom': t.scrollTo({ top: t.scrollHeight, behavior: 'instant' }); break;
  }
  return { y: Math.round(t.scrollTop), height: Math.round(t.scrollHeight), view: Math.round(h) };
}"#;

/// `this` = an element. Its visible text (or value for form fields).
///
/// `innerText` leaves out text inside shadow roots (web components), so when the element has
/// shadow hosts inside, the text is collected from the flat tree instead: shadow roots
/// instead of their hosts' children, slots with what is assigned to them, hidden elements
/// and password values left out, a line break around block elements. Closed shadow roots
/// cannot be read from page scripts; their controls are still in snapshots (the AX tree
/// includes them) and can be acted on.
pub const READ_JS: &str = r#"function() {
  const el = this.nodeType === 1 ? this : this.parentElement;
  if (!el) return this.textContent || '';
  if ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && el.type !== 'password') return el.value;
  const hasShadow = el.shadowRoot || [...el.querySelectorAll('*')].some(e => e.shadowRoot);
  if (!hasShadow) return el.innerText !== undefined ? el.innerText : (el.textContent || '');
  const out = [];
  const walk = n => {
    if (n.nodeType === 3) { out.push(n.data.replace(/\s+/g, ' ')); return; }
    if (n.nodeType !== 1 && n.nodeType !== 11) return;
    let block = false;
    if (n.nodeType === 1) {
      if (['SCRIPT', 'STYLE', 'TEMPLATE', 'NOSCRIPT'].includes(n.tagName)) return;
      const s = getComputedStyle(n);
      if (s.display === 'none' || s.visibility === 'hidden') return;
      if (n.tagName === 'BR') { out.push('\n'); return; }
      if (n.tagName === 'INPUT' || n.tagName === 'TEXTAREA') {
        if (n.type !== 'password' && n.value) out.push(' ' + n.value + ' ');
        return;
      }
      block = !s.display.startsWith('inline');
    }
    let kids = n.shadowRoot ? n.shadowRoot.childNodes : n.childNodes;
    if (n.tagName === 'SLOT') {
      const assigned = n.assignedNodes({ flatten: true });
      if (assigned.length) kids = assigned;
    }
    if (block) out.push('\n');
    for (const k of kids) walk(k);
    if (block) out.push('\n');
  };
  walk(el);
  return out.join('').replace(/[ \t]*\n[ \t]*/g, '\n').replace(/\n{2,}/g, '\n').replace(/ {2,}/g, ' ').trim();
}"#;

/// A resolved target element.
struct Target {
    /// The node's id in its own session (see [`crate::frames`]).
    backend_node_id: i64,
    /// Its child session when it is inside an out-of-process iframe (`None` = the page).
    session: Option<String>,
    /// Its frame slot (0 = the page or a same-process iframe).
    slot: usize,
    object_id: String,
    /// For messages: `@4:e3 button "Sign in"`.
    label: String,
}

impl Target {
    fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }
}

/// Why an element cannot be used right now.
#[derive(Debug, Clone, PartialEq)]
enum Blocker {
    Gone,
    Hidden,
    Disabled,
    ReadOnly,
    NotEditable,
    NoBox,
    /// With details (box position or failed hit test).
    OutsideViewport(String),
    /// Something else is at the click point (`x`, `y` in the page's viewport): node `hit`
    /// of frame `slot`.
    Covered {
        x: f64,
        y: f64,
        hit: i64,
        slot: usize,
    },
}

/// How much checking an action needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Mouse actions: visible, enabled, has a box in the viewport, nothing on top.
    Pointer,
    /// Typing: visible, enabled, editable, not read-only.
    Editable,
    /// Visible and enabled (select).
    Enabled,
}

/// Perform one step on the tab. Does not wait for the page or compute a diff; returns a short
/// line such as `clicked @4:e3 button "Sign in"`.
pub async fn perform(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    step: &Step,
) -> Result<String, CmdError> {
    let value = step.value.as_deref();
    if step.action.sends_input() {
        ensure_page_visible(ctx, tab).await?;
    }
    let need_value = |what: &str| {
        value.ok_or_else(|| {
            CmdError::invalid_params(format!("action {:?} needs a value ({what})", step.action))
        })
    };

    let Some(target_ref) = step.target.as_deref() else {
        // Only press and scroll work without an element.
        return match step.action {
            Action::Press => {
                let chords = keys::parse_keys(need_value("keys such as \"Enter\"")?)?;
                press_keys(ctx, tab, state, &chords).await?;
                Ok(format!("pressed {}", value.unwrap_or_default()))
            }
            Action::Scroll => {
                let dir = scroll_direction(value)?;
                if dir == "into_view" {
                    return Err(CmdError::invalid_params(
                        "scroll without a ref needs value up, down, top, bottom, left or right",
                    ));
                }
                let page = cdp::evaluate_object(
                    ctx,
                    tab,
                    "document.scrollingElement || document.documentElement",
                )
                .await?
                .ok_or_else(|| CmdError::internal("the page has no document element"))?;
                let pos = if state.human.is_some() && matches!(dir, "up" | "down") {
                    wheel_scroll(ctx, tab, state, Scroller::Page(&page), dir).await?
                } else {
                    cdp::call_function(ctx, tab, None, &page, SCROLL_JS, &[json!(dir)]).await?
                };
                Ok(format!("scrolled page {dir} {}", scroll_position(&pos)))
            }
            _ => Err(
                CmdError::invalid_params(format!("action {:?} needs a ref", step.action))
                    .with_hint("Take a page_snapshot and pass ref, e.g. \"@4:e3\"."),
            ),
        };
    };

    let target = resolve_target(ctx, tab, state, target_ref).await?;
    perform_on(ctx, tab, state, &target, step).await
}

/// [`perform`] on a DOM node found some other way than a ref (e.g. Vim-style link hints:
/// the page script points at the element, `DOM.describeNode` gives its `backendNodeId`).
/// Same actionability checks and trusted input as `page.act`; `label` names the element in
/// messages. `step.target` is ignored. The node belongs to the page's own CDP session (not
/// an out-of-process iframe); the hit-test rules (click-through overlays, text nodes, shadow
/// DOM) are the same as for refs.
pub async fn perform_on_node(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    backend_node_id: i64,
    label: &str,
    step: &Step,
) -> Result<String, CmdError> {
    if step.action.sends_input() {
        ensure_page_visible(ctx, tab).await?;
    }
    let object_id = cdp::resolve_object(ctx, tab, None, backend_node_id, label).await?;
    let target = Target {
        backend_node_id,
        session: None,
        slot: 0,
        object_id,
        label: label.to_string(),
    };
    perform_on(ctx, tab, state, &target, step).await
}

/// The element half of [`perform`]: `target` is already resolved.
async fn perform_on(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    target: &Target,
    step: &Step,
) -> Result<String, CmdError> {
    let value = step.value.as_deref();
    let need_value = |what: &str| {
        value.ok_or_else(|| {
            CmdError::invalid_params(format!("action {:?} needs a value ({what})", step.action))
        })
    };
    let done = format!("{} {}", step.action.past_tense(), target.label);
    match step.action {
        Action::Click | Action::Dblclick | Action::Hover => {
            let (_, point) = wait_actionable(ctx, tab, state, target, Need::Pointer).await?;
            let (x, y) = point.expect("pointer checks return a point");
            match step.action {
                Action::Hover => mouse_move(ctx, tab, state, x, y).await?,
                Action::Dblclick => mouse_click(ctx, tab, state, x, y, 2).await?,
                _ => mouse_click(ctx, tab, state, x, y, 1).await?,
            }
            Ok(done)
        }
        Action::Check | Action::Uncheck => {
            let want = step.action == Action::Check;
            let probe = probe_element(ctx, tab, target).await?;
            let Some(current) = probe.checked else {
                return Err(not_interactable(
                    format!("{} has no checked state", target.label),
                    "check/uncheck work on checkboxes, radio buttons and switches; use click \
                     for other elements.",
                ));
            };
            if current == want {
                return Ok(format!("{} was already {}", target.label, on_off(want)));
            }
            if !want && probe.input_type == "radio" {
                return Err(CmdError::invalid_params(format!(
                    "{} is a radio button: it cannot be unchecked",
                    target.label
                ))
                .with_hint("Check another option of the same group instead."));
            }
            let (_, point) = wait_actionable(ctx, tab, state, target, Need::Pointer).await?;
            let (x, y) = point.expect("pointer checks return a point");
            mouse_click(ctx, tab, state, x, y, 1).await?;
            let after = probe_element(ctx, tab, target).await?;
            if after.checked != Some(want) {
                return Err(not_interactable(
                    format!("clicking {} did not make it {}", target.label, on_off(want)),
                    "The page may handle clicks elsewhere; take a page_snapshot and look for \
                     the real control.",
                ));
            }
            Ok(done)
        }
        Action::Fill | Action::Type => {
            let text = need_value("the text to type")?;
            let (probe, _) = wait_actionable(ctx, tab, state, target, Need::Editable).await?;
            focus(ctx, tab, target).await?;
            let (session, object) = (target.session(), target.object_id.as_str());
            if probe.tag == "input" && SET_BY_VALUE_TYPES.contains(&probe.input_type.as_str()) {
                cdp::call_function(ctx, tab, session, object, SET_VALUE_JS, &[json!(text)]).await?;
                return Ok(done);
            }
            if step.action == Action::Fill {
                cdp::call_function(ctx, tab, session, object, SELECT_ALL_JS, &[]).await?;
                if text.is_empty() {
                    // Typing "" would leave the selection; delete it instead.
                    keys::press(ctx, tab, &[keys::parse_chord("Delete")?]).await?;
                }
            }
            if !text.is_empty() {
                type_text(ctx, tab, state, text).await?;
            }
            if step.action == Action::Fill {
                cdp::call_function(ctx, tab, session, object, CHANGE_JS, &[]).await?;
            }
            Ok(done)
        }
        Action::Select => {
            let wanted = need_value("the option's value or label")?;
            wait_actionable(ctx, tab, state, target, Need::Enabled).await?;
            let reply = cdp::call_function(
                ctx,
                tab,
                target.session(),
                &target.object_id,
                SELECT_JS,
                &[json!(wanted)],
            )
            .await?;
            match reply["error"].as_str() {
                None => Ok(format!(
                    "selected \"{}\" in {}",
                    reply["selected"].as_str().unwrap_or(wanted),
                    target.label
                )),
                Some("not_select") => Err(not_interactable(
                    format!("{} is not a <select>", target.label),
                    "For custom dropdowns click the element, then click the option in the diff.",
                )),
                Some("disabled_option") => Err(not_interactable(
                    format!("option \"{wanted}\" of {} is disabled", target.label),
                    "Pick another option.",
                )),
                Some(_) => {
                    let options = reply["options"].as_array().cloned().unwrap_or_default();
                    Err(CmdError::new(
                        ErrorCode::NotFound,
                        format!("{} has no option \"{wanted}\"", target.label),
                    )
                    .with_retry(bmcp_core::Retry::No)
                    .with_hint("Use one of the candidates (value or label).")
                    .with_candidates(options))
                }
            }
        }
        Action::Press => {
            let chords = keys::parse_keys(need_value("keys such as \"Enter\"")?)?;
            focus(ctx, tab, target).await?;
            press_keys(ctx, tab, state, &chords).await?;
            Ok(format!(
                "pressed {} on {}",
                value.unwrap_or_default(),
                target.label
            ))
        }
        Action::Scroll => {
            let dir = scroll_direction(value)?;
            if dir == "into_view" {
                scroll_into_view(ctx, tab, target).await;
                return Ok(format!("scrolled {} into view", target.label));
            }
            let pos = if state.human.is_some() && matches!(dir, "up" | "down") {
                wheel_scroll(ctx, tab, state, Scroller::Element(target), dir).await?
            } else {
                let (session, object) = (target.session(), target.object_id.as_str());
                cdp::call_function(ctx, tab, session, object, SCROLL_JS, &[json!(dir)]).await?
            };
            Ok(format!(
                "scrolled {dir} at {} {}",
                target.label,
                scroll_position(&pos)
            ))
        }
        Action::Focus => {
            focus(ctx, tab, target).await?;
            Ok(done)
        }
    }
}

fn on_off(checked: bool) -> &'static str {
    if checked { "checked" } else { "unchecked" }
}

fn scroll_direction(value: Option<&str>) -> Result<&'static str, CmdError> {
    let dir = value.unwrap_or("into_view").trim().to_ascii_lowercase();
    ["into_view", "up", "down", "top", "bottom", "left", "right"]
        .into_iter()
        .find(|d| *d == dir)
        .ok_or_else(|| {
            CmdError::invalid_params(format!(
                "scroll value '{dir}' is unknown; use into_view, up, down, top, bottom, left or \
                 right"
            ))
        })
}

/// `(y=1200/5400)`: scroll offset / scroll height after scrolling.
fn scroll_position(pos: &Value) -> String {
    format!(
        "(y={}/{})",
        pos["y"].as_i64().unwrap_or(0),
        pos["height"].as_i64().unwrap_or(0)
    )
}

/// Chromium silently drops mouse/keyboard input for hidden pages (window minimized, or fully
/// covered by other windows when Windows occlusion tracking is on). Say so instead of
/// "clicking" without effect.
pub async fn ensure_page_visible(ctx: &Ctx, tab: TabId) -> Result<(), CmdError> {
    let state = cdp::evaluate(ctx, tab, "document.visibilityState").await?;
    if state.as_str() == Some("hidden") {
        return Err(not_interactable(
            format!("tab {tab} is hidden, so the browser ignores input to it"),
            "Activate the tab and make sure the BrowserMCP window is not minimized or fully \
             covered by other windows, then retry.",
        ));
    }
    Ok(())
}

pub fn not_interactable(message: impl Into<String>, hint: &str) -> CmdError {
    CmdError::new(ErrorCode::NotInteractable, message).with_hint(hint)
}

/// Ref → DOM node → JavaScript handle, with a label for messages.
async fn resolve_target(
    ctx: &Ctx,
    tab: TabId,
    state: &TabState,
    target_ref: &str,
) -> Result<Target, CmdError> {
    // The id in the merged tree (snapshot refs), then where the node really lives.
    let merged = page::resolve_ref(ctx, tab, state, target_ref)?;
    let at = state.frames.locate(merged);
    let label = match state.describe(merged) {
        Some(desc) => format!("{} {desc}", target_ref.trim()),
        None => target_ref.trim().to_string(),
    };
    let object_id =
        cdp::resolve_object(ctx, tab, at.session.as_deref(), at.backend, &label).await?;
    Ok(Target {
        backend_node_id: at.backend,
        session: at.session,
        slot: at.slot,
        object_id,
        label,
    })
}

async fn probe_element(ctx: &Ctx, tab: TabId, target: &Target) -> Result<Probe, CmdError> {
    let value =
        cdp::call_function(ctx, tab, target.session(), &target.object_id, PROBE_JS, &[]).await?;
    let probe: Probe = serde_json::from_value(value).unwrap_or_default();
    if !probe.connected {
        return Err(cdp::node_gone(&target.label));
    }
    Ok(probe)
}

async fn focus(ctx: &Ctx, tab: TabId, target: &Target) -> Result<(), CmdError> {
    let reply = ctx
        .backend
        .cdp_session(
            tab,
            target.session.clone(),
            "DOM.focus".into(),
            json!({ "backendNodeId": target.backend_node_id }),
        )
        .await;
    match reply {
        Ok(_) => Ok(()),
        Err(bmcp_core::BackendError::Cdp(msg)) => Err(not_interactable(
            format!("{} cannot take keyboard focus ({msg})", target.label),
            "Click the element instead, or pick the real input field from a page_snapshot.",
        )),
        Err(other) => Err(other.into()),
    }
}

/// `DOM.scrollIntoViewIfNeeded`. Failure (no layout box) is not an error here: the box check
/// that follows reports it better.
/// In an out-of-process iframe Chromium also scrolls the frames around it.
async fn scroll_into_view(ctx: &Ctx, tab: TabId, target: &Target) {
    let _ = ctx
        .backend
        .cdp_session(
            tab,
            target.session.clone(),
            "DOM.scrollIntoViewIfNeeded".into(),
            json!({ "backendNodeId": target.backend_node_id }),
        )
        .await;
}

/// Check the element again and again until it is usable or [`ACTIONABLE_WAIT`] is over.
/// Returns the probe and, for [`Need::Pointer`], the point to click.
async fn wait_actionable(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    target: &Target,
    need: Need,
) -> Result<(Probe, Option<(f64, f64)>), CmdError> {
    let started = tokio::time::Instant::now();
    loop {
        let blocker = match check_once(ctx, tab, &state.frames, target, need).await? {
            Ok(found) => return Ok(found),
            Err(blocker) => blocker,
        };
        // Waiting does not help for these.
        let permanent = matches!(
            blocker,
            Blocker::Gone | Blocker::NotEditable | Blocker::ReadOnly
        );
        if permanent || started.elapsed() >= ACTIONABLE_WAIT {
            return Err(blocker_error(ctx, tab, state, target, blocker).await);
        }
        tokio::time::sleep(RETRY_PAUSE).await;
    }
}

/// One round of actionability checks. For an element inside an out-of-process iframe they run
/// in the frame's session; the returned point is always in the page's viewport.
async fn check_once(
    ctx: &Ctx,
    tab: TabId,
    frames: &Frames,
    target: &Target,
    need: Need,
) -> Result<Result<(Probe, Option<(f64, f64)>), Blocker>, CmdError> {
    let session = target.session();
    let value = cdp::call_function(ctx, tab, session, &target.object_id, PROBE_JS, &[]).await?;
    let probe: Probe = serde_json::from_value(value).unwrap_or_default();
    if !probe.connected {
        return Ok(Err(Blocker::Gone));
    }
    if probe.hidden {
        return Ok(Err(Blocker::Hidden));
    }
    if probe.disabled {
        return Ok(Err(Blocker::Disabled));
    }
    if need == Need::Editable {
        if !probe.editable {
            return Ok(Err(Blocker::NotEditable));
        }
        if probe.readonly {
            return Ok(Err(Blocker::ReadOnly));
        }
    }

    scroll_into_view(ctx, tab, target).await;
    let Some(rect) = cdp::content_box(ctx, tab, session, target.backend_node_id).await? else {
        return Ok(Err(Blocker::NoBox));
    };
    if need != Need::Pointer {
        return Ok(Ok((probe, None)));
    }

    // Click the middle of the part that is inside the viewport (the frame's own viewport for
    // an out-of-process iframe; `rect` is in the same coordinates).
    let layout = cdp::layout(ctx, tab, session).await?;
    let Some(visible) = clip_to_viewport(rect, layout.viewport_width, layout.viewport_height)
    else {
        return Ok(Err(Blocker::OutsideViewport(format!(
            "its box at ({:.0}, {:.0}) is outside the {:.0}x{:.0} viewport",
            rect.x, rect.y, layout.viewport_width, layout.viewport_height
        ))));
    };
    let (x, y) = visible.center();

    // Hit test: which node would receive a click at (x, y)? Unlike the quads and mouse events
    // (viewport coordinates), `DOM.getNodeForLocation` takes document coordinates, so add the
    // scroll offset (verified live on a scrolled page).
    let hit = match hit_test(ctx, tab, session, x + layout.page_x, y + layout.page_y).await? {
        Ok(hit) => hit,
        Err(msg) => {
            return Ok(Err(Blocker::OutsideViewport(format!(
                "nothing is hit at ({x:.0}, {y:.0}) {msg}"
            ))));
        }
    };
    // Mouse events go to the page, so the point to click is in the page's viewport.
    let (dx, dy) = frames::frame_offset(ctx, tab, frames, target.slot).await?;
    let (px, py) = (x + dx, y + dy);
    if hit != target.backend_node_id && !reaches(ctx, tab, session, &target.object_id, hit).await? {
        return Ok(Err(Blocker::Covered {
            x: px,
            y: py,
            hit,
            slot: target.slot,
        }));
    }
    // Inside an out-of-process iframe: the page itself must not cover the frame there.
    if let Some(owner) = frames.outermost_owner(target.slot) {
        let page = cdp::layout(ctx, tab, None).await?;
        let top = match hit_test(ctx, tab, None, px + page.page_x, py + page.page_y).await? {
            Ok(top) => top,
            Err(msg) => {
                return Ok(Err(Blocker::OutsideViewport(format!(
                    "nothing is hit at ({px:.0}, {py:.0}) {msg}"
                ))));
            }
        };
        if top != owner {
            let iframe = cdp::resolve_object(ctx, tab, None, owner, "the iframe").await?;
            if !reaches(ctx, tab, None, &iframe, top).await? {
                return Ok(Err(Blocker::Covered {
                    x: px,
                    y: py,
                    hit: top,
                    slot: 0,
                }));
            }
        }
    }
    Ok(Ok((probe, Some((px, py)))))
}

/// `DOM.getNodeForLocation` at document coordinates of a session (`None` = the page). The
/// inner `Err` holds Chromium's message when nothing is there.
///
/// `ignorePointerEventsNone: false` makes the hit test skip `pointer-events: none` elements,
/// exactly like a real mouse event does: a decorative overlay or fade that clicks pass
/// through does not count as covering the target. Nodes in (open or closed) shadow roots are
/// returned as they are; [`HIT_OK_JS`] walks up through their hosts.
async fn hit_test(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    x: f64,
    y: f64,
) -> Result<Result<i64, String>, CmdError> {
    let reply = ctx
        .backend
        .cdp_session(
            tab,
            session.map(str::to_string),
            "DOM.getNodeForLocation".into(),
            json!({
                "x": x.floor() as i64,
                "y": y.floor() as i64,
                "includeUserAgentShadowDOM": false,
                "ignorePointerEventsNone": false,
            }),
        )
        .await;
    match reply {
        Ok(reply) => Ok(reply["backendNodeId"].as_i64().ok_or_else(String::new)),
        // "No node found at given location"
        Err(bmcp_core::BackendError::Cdp(msg)) => Ok(Err(msg)),
        Err(other) => Err(other.into()),
    }
}

/// Does a click that lands on node `hit` reach the element `object` (see [`HIT_OK_JS`])?
async fn reaches(
    ctx: &Ctx,
    tab: TabId,
    session: Option<&str>,
    object: &str,
    hit: i64,
) -> Result<bool, CmdError> {
    let hit_object = cdp::resolve_object(ctx, tab, session, hit, "the element on top").await?;
    let reply = cdp::call_on(
        ctx,
        tab,
        session,
        object,
        HIT_OK_JS,
        vec![cdp::arg_object(&hit_object)],
        true,
    )
    .await?;
    Ok(reply["value"].as_bool() == Some(true))
}

/// The part of `rect` inside a `width`×`height` viewport, if any.
fn clip_to_viewport(rect: BoxRect, width: f64, height: f64) -> Option<BoxRect> {
    // Unknown viewport size (metrics failed): trust the element box.
    if width <= 0.0 || height <= 0.0 {
        return Some(rect);
    }
    let left = rect.x.max(0.0);
    let top = rect.y.max(0.0);
    let right = (rect.x + rect.width).min(width);
    let bottom = (rect.y + rect.height).min(height);
    if right - left < 1.0 || bottom - top < 1.0 {
        return None;
    }
    Some(BoxRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// Turn a blocker into the error the agent sees.
async fn blocker_error(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    target: &Target,
    blocker: Blocker,
) -> CmdError {
    let label = &target.label;
    match blocker {
        Blocker::Gone => cdp::node_gone(label),
        Blocker::Hidden => not_interactable(
            format!("{label} is hidden (display:none or visibility:hidden)"),
            "It may appear after another action (open a menu, expand a section); take a \
             page_snapshot.",
        ),
        Blocker::Disabled => not_interactable(
            format!("{label} is disabled"),
            "Fill in the required fields first, or wait (nav_wait) until it is enabled.",
        ),
        Blocker::ReadOnly => {
            not_interactable(format!("{label} is read-only"), "Pick another field.")
        }
        Blocker::NotEditable => not_interactable(
            format!("{label} is not a text field"),
            "fill/type work on text inputs, textareas and contenteditable elements; use select \
             for <select>, check for checkboxes, click for buttons.",
        ),
        Blocker::NoBox => not_interactable(
            format!("{label} is not visible (it has no box on the page)"),
            "It may be collapsed or not rendered yet; take a page_snapshot.",
        ),
        Blocker::OutsideViewport(detail) => not_interactable(
            format!("{label} could not be scrolled into the visible area: {detail}"),
            "Try page_act with action scroll, then retry.",
        ),
        Blocker::Covered { x, y, hit, slot } => {
            covered_error(ctx, tab, state, target, (x, y), (hit, slot))
                .await
                .unwrap_or_else(|err| {
                    // Describing the cover failed; still report the main fact.
                    not_interactable(
                        format!("{label} is covered by another element at ({x:.0}, {y:.0})"),
                        &format!(
                            "Close what is on top first (details failed: {})",
                            err.message
                        ),
                    )
                })
        }
    }
}

/// `NOT_INTERACTABLE` that names what covers the target and offers refs to deal with it, e.g.
/// `covered by dialog "Cookies" (div#cookie-banner)` + `button "Accept all" @5:e9`.
///
/// `(x, y)` is the click point in the page's viewport; `hit` is the node found there, in
/// frame `slot` (see [`crate::frames`]).
async fn covered_error(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    target: &Target,
    (x, y): (f64, f64),
    (hit, slot): (i64, usize),
) -> Result<CmdError, CmdError> {
    let session = state.frames.session_of(slot);
    let session = session.as_deref();
    let hit_object = cdp::resolve_object(ctx, tab, session, hit, "the element on top").await?;
    let cover_object = cdp::call_function_object(ctx, tab, session, &hit_object, COVER_JS)
        .await?
        .unwrap_or(hit_object);
    let css = cdp::call_function(ctx, tab, session, &cover_object, CSS_NAME_JS, &[])
        .await?
        .as_str()
        .unwrap_or("element")
        .to_string();
    let described = cdp::call_in(
        ctx,
        tab,
        session,
        "DOM.describeNode",
        json!({ "objectId": cover_object, "pierce": true }),
    )
    .await?;
    // The cover's id in the merged tree the snapshot below is built from.
    let cover = described["node"]["backendNodeId"].as_i64().unwrap_or(hit);
    let cover = Frames::merged_id(slot, cover);

    // A fresh snapshot gives the agent refs for the controls inside the cover.
    let (snap, tree) =
        page::take_snapshot_with_tree(ctx, tab, state, bmcp_perception::SnapshotOptions::default())
            .await?;

    // Role and name of the cover from the accessibility tree, e.g. `dialog "Cookies"`.
    let mut name = css.clone();
    if let Some(index) = tree.index_of_backend(cover) {
        let node = &tree.nodes[index];
        let role = node.role();
        if !matches!(role, "generic" | "none" | "") || !node.name().is_empty() {
            name = if node.name().is_empty() {
                format!("{role} ({css})")
            } else {
                format!("{role} \"{}\" ({css})", node.name())
            };
        }
    }

    // Refs printed in the snapshot whose node lies inside the cover.
    let inside = |backend: i64| -> bool {
        let mut index = tree.index_of_backend(backend);
        while let Some(i) = index {
            if tree.nodes[i].backend_dom_node_id == Some(cover) {
                return true;
            }
            index = tree.parent(i);
        }
        false
    };
    let controls: Vec<String> = snap
        .refs
        .iter()
        .filter(|r| inside(r.backend_node_id))
        .take(5)
        .map(|r| {
            if r.name.is_empty() {
                format!("{} {}", r.role, r.ref_str)
            } else {
                format!("{} \"{}\" {}", r.role, r.name, r.ref_str)
            }
        })
        .collect();
    let hint = if controls.is_empty() {
        format!("Close or remove {name} first (take a page_snapshot to find its controls).")
    } else {
        format!(
            "Deal with {name} first, e.g. page_act on one of: {}.",
            controls.join(", ")
        )
    };
    Ok(not_interactable(
        format!("{} is covered by {name} at ({x:.0}, {y:.0})", target.label),
        &hint,
    ))
}

// ---------------------------------------------------------------- input: fast or human-like
//
// Each helper sends the minimal events on the fast path (default) and the human-like
// sequence when `input.humanlike` is on for the tab (see `crate::human`).

/// Sleep about `mean_ms` in human mode; nothing on the fast path.
async fn human_pause(state: &mut TabState, mean_ms: f64, spread: f64) {
    if let Some(h) = &mut state.human {
        let pause = h.rng.millis(mean_ms, spread);
        tokio::time::sleep(pause).await;
    }
}

async fn send_mouse_moved(ctx: &Ctx, tab: TabId, x: f64, y: f64) -> Result<(), CmdError> {
    cdp::call(
        ctx,
        tab,
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
    )
    .await?;
    Ok(())
}

/// Move the mouse to (x, y): one event, or a human-like path from the last position.
async fn mouse_move(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    x: f64,
    y: f64,
) -> Result<(), CmdError> {
    let last = state.mouse;
    state.mouse = Some((x, y));
    let Some(h) = &mut state.human else {
        return send_mouse_moved(ctx, tab, x, y).await;
    };
    let from = match last {
        Some(point) => point,
        None => {
            // First move in this tab: come from somewhere plausible in the viewport.
            let layout = cdp::layout(ctx, tab, None).await?;
            let viewport = (layout.viewport_width, layout.viewport_height);
            human::entry_point((x, y), viewport, &mut h.rng)
        }
    };
    for step in human::mouse_path(from, (x, y), &h.profile, &mut h.rng) {
        tokio::time::sleep(step.delay).await;
        send_mouse_moved(ctx, tab, step.x, step.y).await?;
    }
    Ok(())
}

/// Move there, then press and release the left button `clicks` times (2 = double click).
async fn mouse_click(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    x: f64,
    y: f64,
    clicks: u32,
) -> Result<(), CmdError> {
    // People do not hit the exact centre (stays within 1.5 px, so small targets still work).
    let (x, y) = match &mut state.human {
        Some(h) => human::aim((x, y), 1.5, &h.profile, &mut h.rng),
        None => (x, y),
    };
    mouse_move(ctx, tab, state, x, y).await?;
    for count in 1..=clicks {
        // Dwell before the first press; the gap between the clicks of a double click.
        human_pause(state, if count == 1 { 90.0 } else { 110.0 }, 0.4).await;
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            if kind == "mouseReleased"
                && let Some(h) = &mut state.human
            {
                let hold = human::hold_time(&mut h.rng);
                tokio::time::sleep(hold).await;
            }
            cdp::call(
                ctx,
                tab,
                "Input.dispatchMouseEvent",
                json!({
                    "type": kind, "x": x, "y": y,
                    "button": "left", "buttons": buttons, "clickCount": count,
                }),
            )
            .await?;
        }
    }
    Ok(())
}

/// Type text into the focused field: one trusted `Input.insertText` (fast path), or key
/// events per character with typing delays (human-like). Characters without a key on a US
/// keyboard (accents, emoji, newlines) are inserted as text.
async fn type_text(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    text: &str,
) -> Result<(), CmdError> {
    if state.human.is_none() {
        // Trusted text input: fires beforeinput/input like real typing.
        cdp::call(ctx, tab, "Input.insertText", json!({ "text": text })).await?;
        return Ok(());
    }
    let mut prev = None;
    for c in text.chars() {
        let Some(h) = &mut state.human else { break };
        let delay = human::key_delay(prev, &h.profile, &mut h.rng);
        let hold = human::hold_time(&mut h.rng);
        tokio::time::sleep(delay).await;
        match keys::char_chord(c) {
            Some(chord) => {
                let release = chord.modifier_keys.len() + 1;
                for (i, event) in keys::chord_events(&chord).into_iter().enumerate() {
                    if i == release {
                        tokio::time::sleep(hold).await;
                    }
                    cdp::call(ctx, tab, "Input.dispatchKeyEvent", event).await?;
                }
            }
            None => {
                cdp::call(
                    ctx,
                    tab,
                    "Input.insertText",
                    json!({ "text": c.to_string() }),
                )
                .await?;
            }
        }
        prev = Some(c);
    }
    Ok(())
}

/// Press chords; in human mode with a pause before each.
async fn press_keys(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    chords: &[keys::Chord],
) -> Result<(), CmdError> {
    for chord in chords {
        human_pause(state, 140.0, 0.4).await;
        keys::press(ctx, tab, std::slice::from_ref(chord)).await?;
    }
    Ok(())
}

/// What a human-like scroll turns the wheel over.
enum Scroller<'a> {
    /// The page's scrolling element (an object id in the page).
    Page(&'a str),
    /// An element (perhaps inside an out-of-process iframe).
    Element(&'a Target),
}

/// Human-like scroll up/down: move the mouse over the element (or keep it over the page) and
/// turn the wheel in notches. The scroll position is read like [`SCROLL_JS`] does.
async fn wheel_scroll(
    ctx: &Ctx,
    tab: TabId,
    state: &mut TabState,
    scroller: Scroller<'_>,
    dir: &str,
) -> Result<Value, CmdError> {
    let (session, object_id) = match &scroller {
        Scroller::Page(object) => (None, *object),
        Scroller::Element(target) => (target.session(), target.object_id.as_str()),
    };
    let before =
        cdp::call_function(ctx, tab, session, object_id, SCROLL_JS, &[json!("none")]).await?;
    let view = before["view"].as_f64().unwrap_or(600.0).max(50.0);
    let layout = cdp::layout(ctx, tab, None).await?;
    let vw = layout.viewport_width.max(1.0);
    let vh = layout.viewport_height.max(1.0);
    // Where the wheel turns: over the element, else where the mouse is (or the middle).
    let element = match &scroller {
        Scroller::Element(target) => {
            let (dx, dy) = frames::frame_offset(ctx, tab, &state.frames, target.slot).await?;
            cdp::content_box(ctx, tab, session, target.backend_node_id)
                .await?
                .map(|r| BoxRect {
                    x: r.x + dx,
                    y: r.y + dy,
                    ..r
                })
                .and_then(|r| clip_to_viewport(r, vw, vh))
                .map(|r| r.center())
        }
        Scroller::Page(_) => None,
    };
    let (x, y) = element.or(state.mouse).unwrap_or((vw / 2.0, vh / 2.0));
    mouse_move(ctx, tab, state, x, y).await?;
    let total = if dir == "up" { -0.8 * view } else { 0.8 * view };
    let steps = match &mut state.human {
        Some(h) => human::wheel_steps(total, &h.profile, &mut h.rng),
        None => vec![(total, Duration::ZERO)],
    };
    for (delta, pause) in steps {
        tokio::time::sleep(pause).await;
        cdp::call(
            ctx,
            tab,
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": 0, "deltaY": delta }),
        )
        .await?;
    }
    // Smooth scrolling needs a moment before the position is final.
    tokio::time::sleep(Duration::from_millis(150)).await;
    cdp::call_function(ctx, tab, session, object_id, SCROLL_JS, &[json!("none")]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_boxes_to_the_viewport() {
        let inside = BoxRect {
            x: 10.0,
            y: 10.0,
            width: 100.0,
            height: 20.0,
        };
        assert_eq!(clip_to_viewport(inside, 800.0, 600.0), Some(inside));
        let tall = BoxRect {
            x: 0.0,
            y: -100.0,
            width: 200.0,
            height: 1000.0,
        };
        let clipped = clip_to_viewport(tall, 800.0, 600.0).unwrap();
        assert_eq!(clipped.center(), (100.0, 300.0));
        let below = BoxRect {
            x: 0.0,
            y: 700.0,
            width: 10.0,
            height: 10.0,
        };
        assert_eq!(clip_to_viewport(below, 800.0, 600.0), None);
    }

    #[test]
    fn scroll_directions() {
        assert_eq!(scroll_direction(None).unwrap(), "into_view");
        assert_eq!(scroll_direction(Some("Down")).unwrap(), "down");
        assert!(scroll_direction(Some("sideways")).is_err());
    }
}
