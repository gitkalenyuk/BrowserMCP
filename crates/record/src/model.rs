//! What a recording is made of: one [`Step`] per line of `actions.jsonl`, plus [`RecordingMeta`]
//! in `meta.json`.
//!
//! A step is either an API call (`source: "api"`, from the `command.started/finished` events of
//! the registry) or something a person did in the page (`source: "human"`, from the injected
//! recorder script). When an API call itself causes page events (a `page.act` click is a real
//! trusted click, so the recorder sees it too), the human copy is kept for debugging but marked
//! `dup_of: <seq of the API step>`; codegen skips it.

use bmcp_core::TabId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a step came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A command call through the registry (MCP, REST, WS, script, UI…).
    #[default]
    Api,
    /// An event seen by the recorder script in the page (mouse, keyboard, navigation).
    Human,
    /// Written by the recorder itself (the `start` step with the first URL).
    System,
}

/// How to find an element again in a later run. Several ways are stored; codegen prefers
/// role + name (what `page.find` searches), then text, and keeps `css` only as a comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Target {
    /// The ref the API call used (`@4:e3`). Only meaningful during the recording.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    /// Accessible role (`textbox`, `button`, `link`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Accessible name (`"Email"`, `"Sign in"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Visible text of the element (first 80 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 0-based index among elements with the same role and name (only stored when > 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,
    /// CSS path fallback (`#login > p > button`), from the recorder script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// HTML tag (`input`), from the recorder script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// `<input type=…>`, from the recorder script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    /// Where role/name came from: `snapshot` (perception history), `ax` (re-queried
    /// accessibility node) or `dom` (computed by the recorder script).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

impl Target {
    /// Fill the fields that are still empty from `other` (keeps what we already know).
    pub fn merge_missing(&mut self, other: &Target) {
        fn fill<T: Clone>(mine: &mut Option<T>, theirs: &Option<T>) {
            if mine.is_none() {
                mine.clone_from(theirs);
            }
        }
        fill(&mut self.ref_, &other.ref_);
        if self.role.is_none() && self.name.is_none() {
            self.role.clone_from(&other.role);
            self.name.clone_from(&other.name);
            self.nth = other.nth;
            self.from.clone_from(&other.from);
        }
        fill(&mut self.text, &other.text);
        fill(&mut self.css, &other.css);
        fill(&mut self.tag, &other.tag);
        fill(&mut self.input_type, &other.input_type);
    }

    /// True when it says how to find the element without a ref.
    pub fn has_locator(&self) -> bool {
        self.role.is_some() || self.name.is_some() || self.text.is_some()
    }
}

/// The API call behind an `api` step.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandCall {
    /// Dotted command name (`page.act`).
    pub name: String,
    /// Registry call number; the same in `command.started` and `command.finished`.
    pub call_id: u64,
    /// Who called (`{"kind":"ws","conn":1}`).
    pub caller: Value,
    /// Arguments as sent (secrets replaced by `"***"`).
    pub args: Value,
    /// Filled in from `command.finished`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// Error code when the call failed (`STALE_REF`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// One line of `actions.jsonl`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Step {
    /// 1, 2, 3… in the order the recorder saw things.
    pub seq: u64,
    /// Wall-clock time (ms since the Unix epoch).
    pub ts_ms: u64,
    pub source: Source,
    /// `start`, `goto`, `back`, `forward`, `reload`, `wait`, `open_tab`, `navigated`,
    /// `command` (any other API call), or an element action: `click`, `dblclick`, `fill`,
    /// `type`, `select`, `check`, `uncheck`, `hover`, `press`, `scroll`, `focus`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// Text typed, option chosen, keys pressed, scroll direction or wait value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The value is a secret (password field): stored as `"***"`, never put in scripts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
    /// URL for `start`/`goto`/`navigated`/`open_tab`, else the page URL at the time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// For `wait`: `load`, `text`, `ref` or `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_for: Option<String>,
    /// For `source: "api"`: the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandCall>,
    /// This human event was caused by the API step with this `seq` (skipped by codegen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dup_of: Option<u64>,
    /// Extra facts: `scroll_y`, the option label of a select, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
    /// API step whose `command.finished` has not arrived yet. Such steps are not written to
    /// disk (their secret flag is not known yet).
    #[serde(skip)]
    pub pending: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Step {
    /// True for steps that do something to an element (click, fill, …).
    pub fn is_element_action(&self) -> bool {
        matches!(
            self.action.as_str(),
            "click"
                | "dblclick"
                | "fill"
                | "type"
                | "select"
                | "check"
                | "uncheck"
                | "hover"
                | "press"
                | "scroll"
                | "focus"
        )
    }

    /// An API call that failed (codegen leaves it out).
    pub fn failed(&self) -> bool {
        self.command.as_ref().and_then(|c| c.ok) == Some(false)
    }
}

/// `meta.json` of a recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingMeta {
    /// Folder name under `data/recordings/`, e.g. `login-20261106-125600`.
    pub id: String,
    /// Name given to `record.start` (also the default script name).
    pub name: String,
    pub tab: TabId,
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_ms: Option<u64>,
    /// URL of the tab when recording started.
    pub start_url: String,
    /// Number of steps written.
    pub steps: usize,
    /// How human actions were captured: `binding` (CDP `Runtime.bindingCalled`), `poll`
    /// (queue in the isolated world read every 250 ms), `pending` (not known yet) or
    /// `failed: …`.
    pub human_capture: String,
    /// `video.webm`/`video.mp4` inside the recording folder, when `video: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    /// `traffic.jsonl` inside the recording folder, when `traffic: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<String>,
}

impl Default for RecordingMeta {
    fn default() -> Self {
        RecordingMeta {
            id: String::new(),
            name: String::new(),
            tab: TabId(0),
            started_ms: 0,
            stopped_ms: None,
            start_url: String::new(),
            steps: 0,
            human_capture: String::new(),
            video: None,
            traffic: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip_and_omit_empty_fields() {
        let step = Step {
            seq: 3,
            ts_ms: 10,
            source: Source::Human,
            action: "click".into(),
            target: Some(Target {
                role: Some("button".into()),
                name: Some("Sign in".into()),
                ..Target::default()
            }),
            ..Step::default()
        };
        let line = serde_json::to_string(&step).unwrap();
        assert_eq!(
            line,
            r#"{"seq":3,"ts_ms":10,"source":"human","action":"click","target":{"role":"button","name":"Sign in"}}"#
        );
        let back: Step = serde_json::from_str(&line).unwrap();
        assert_eq!(back, step);
    }

    #[test]
    fn merge_keeps_known_role_and_adds_css() {
        let mut api = Target {
            ref_: Some("@3:e2".into()),
            role: Some("textbox".into()),
            name: Some("Email".into()),
            from: Some("snapshot".into()),
            ..Target::default()
        };
        let dom = Target {
            role: Some("textbox".into()),
            name: Some("E-mail".into()),
            css: Some("#email".into()),
            input_type: Some("email".into()),
            from: Some("dom".into()),
            ..Target::default()
        };
        api.merge_missing(&dom);
        assert_eq!(api.name.as_deref(), Some("Email"));
        assert_eq!(api.css.as_deref(), Some("#email"));
        assert_eq!(api.input_type.as_deref(), Some("email"));
        assert_eq!(api.from.as_deref(), Some("snapshot"));
    }
}
