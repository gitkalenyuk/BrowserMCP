//! Recording → TypeScript script for `bmcp-script-host` (deterministic: same steps, same text).
//!
//! The script finds elements again by role + accessible name (`page.find`), never by the refs
//! of the recording (refs only live as long as one page). A short helper prelude keeps every
//! step one line:
//!
//! ```ts
//! await browser.nav.goto({ url: "http://127.0.0.1:8000/login.html" });
//! await act("textbox", "Email", "fill", inputs.email ?? "ann@example.test");
//! await act("textbox", "Password", "fill", need("password"));
//! await act("button", "Sign in", "click");
//! ```
//!
//! With `parametrize`, typed and selected values become `inputs.<name>` (name from the field
//! label: `email`, `password`, `query`, …) with the recorded value as default. Secrets are
//! never written into the script: they always become a required input (`need("password")`).

use std::collections::HashMap;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::api;
use crate::model::{RecordingMeta, Source, Step, Target};

/// Options of [`generate`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Typed/selected values become `inputs.<name> ?? "<recorded value>"`.
    pub parametrize: bool,
}

/// An input the script reads (`--input name=value`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct InputSpec {
    pub name: String,
    /// The label of the field it goes into.
    pub label: String,
    /// The recorded value (`None` for secrets).
    pub default: Option<String>,
    /// Required, never stored (passwords).
    pub secret: bool,
}

/// The generated script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub code: String,
    pub inputs: Vec<InputSpec>,
}

const HELPERS: &str = r#"// ---- helpers (generated; edit freely) -------------------------------------------

type Query = { role?: string; name?: string; text?: string };

/** The ref of the nth (0-based) element matching `query`; waits up to 10 s for it. */
async function locate(query: Query, nth = 0): Promise<string> {
  const until = Date.now() + 10_000;
  let last = "";
  for (;;) {
    try {
      const found = (await browser.page.find({ ...query, limit: nth + 1 })) as { text: string };
      const refs = found.text.match(/@\d+:e\d+/g) ?? [];
      if (refs.length > nth) return refs[nth];
      last = found.text;
    } catch (e) {
      last = String(e); // e.g. the page is navigating: try again
    }
    if (Date.now() > until) {
      throw new Error(`element not found: ${JSON.stringify(query)} #${nth}\n${last}`);
    }
    await sleep(250);
  }
}

/** Act (click, fill, select, check, press, …) on the element matching `query`. */
async function actBy(query: Query, action: string, value?: string, nth = 0) {
  const ref = await locate(query, nth);
  const args: Record<string, unknown> = { ref, action, returns: "none" };
  if (value !== undefined) args.value = value;
  await browser.page.act(args);
}

/** Act on the element with this role and accessible name. */
async function act(role: string, name: string, action: string, value?: string, nth = 0) {
  await actBy({ role, name }, action, value, nth);
}

/** A required input. Secrets are never written into scripts. */
function need(key: string): string {
  const value = inputs[key];
  if (value === undefined || value === "") throw new Error(`missing input: --input ${key}=...`);
  return value;
}

// ---- steps -------------------------------------------------------------------------
"#;

/// A navigation this soon after a click/key is caused by it.
const NAV_AFTER_ACTION_MS: u64 = 5000;

/// A JavaScript string literal (JSON strings are valid JS).
fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Builds the step lines and collects inputs.
struct Writer {
    options: Options,
    lines: Vec<String>,
    inputs: Vec<InputSpec>,
    /// (input key, recorded value) → input name, so the same field+value reuses its input.
    input_names: HashMap<(String, String), String>,
    /// What the last emitted line did: for navigation handling and goto dedupe.
    last: Last,
}

#[derive(Debug, Clone, PartialEq)]
enum Last {
    Nothing,
    /// A goto (or the start) of this URL.
    Goto(String),
    /// An element action; its target key, line index (to fold click + dblclick) and time.
    Element {
        action: String,
        key: String,
        line: usize,
        ts_ms: u64,
    },
    Other,
}

/// Identity of a target for "same element" checks.
fn target_key(t: &Target) -> String {
    format!(
        "{}|{}|{}|{}",
        t.role.as_deref().unwrap_or(""),
        t.name.as_deref().unwrap_or(""),
        t.nth.unwrap_or(0),
        t.css.as_deref().unwrap_or("")
    )
}

/// A readable label for comments and input specs.
fn label_of(t: &Target) -> String {
    t.name
        .clone()
        .or_else(|| t.text.clone())
        .or_else(|| t.css.clone())
        .unwrap_or_default()
}

/// `"First name"` → `"firstName"`; well-known fields get their usual names.
fn input_base_name(target: Option<&Target>, secret: bool) -> String {
    let label = target.map(label_of).unwrap_or_default();
    let l = label.to_lowercase();
    let role = target.and_then(|t| t.role.as_deref()).unwrap_or("");
    let input_type = target.and_then(|t| t.input_type.as_deref()).unwrap_or("");
    if secret || l.contains("password") || input_type == "password" {
        return "password".into();
    }
    if l.contains("email") || l.contains("e-mail") || input_type == "email" {
        return "email".into();
    }
    if role == "searchbox" || l.contains("search") || input_type == "search" {
        return "query".into();
    }
    if l.contains("username") || l.contains("user name") || l == "login" {
        return "username".into();
    }
    if l.contains("phone") || input_type == "tel" {
        return "phone".into();
    }
    let mut out = String::new();
    for word in l
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if out.is_empty() {
            out.push_str(word);
        } else {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(chars.as_str());
            }
        }
        if out.len() >= 30 {
            break;
        }
    }
    if out.is_empty() {
        "field".into()
    } else if out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("field{out}")
    } else {
        out
    }
}

impl Writer {
    fn push(&mut self, line: String, last: Last) {
        self.lines.push(line);
        self.last = last;
    }

    fn comment(&mut self, text: &str) {
        self.lines.push(format!("// {text}"));
    }

    /// A unique input name for this target/value.
    fn input(&mut self, target: Option<&Target>, value: &str, secret: bool) -> String {
        let key = target.map(target_key).unwrap_or_default();
        let memo = (key, if secret { String::new() } else { value.into() });
        if let Some(name) = self.input_names.get(&memo) {
            return name.clone();
        }
        let base = input_base_name(target, secret);
        let mut name = base.clone();
        let mut n = 2;
        while self.inputs.iter().any(|i| i.name == name) {
            name = format!("{base}{n}");
            n += 1;
        }
        self.inputs.push(InputSpec {
            name: name.clone(),
            label: target.map(label_of).unwrap_or_default(),
            default: (!secret).then(|| value.to_string()),
            secret,
        });
        self.input_names.insert(memo, name.clone());
        name
    }

    /// The expression for a typed/selected value.
    fn value_expr(&mut self, step: &Step) -> String {
        let value = step.value.clone().unwrap_or_default();
        if step.secret {
            let name = self.input(step.target.as_ref(), "", true);
            return format!("need({})", js(&name));
        }
        if self.options.parametrize {
            let name = self.input(step.target.as_ref(), &value, false);
            return format!("inputs.{name} ?? {}", js(&value));
        }
        js(&value)
    }

    fn goto(&mut self, url: &str) {
        if self.last == Last::Goto(url.to_string()) {
            return;
        }
        self.push(
            format!("await browser.nav.goto({{ url: {} }});", js(url)),
            Last::Goto(url.to_string()),
        );
    }

    fn step(&mut self, step: &Step) {
        match step.action.as_str() {
            "start" => {
                if let Some(url) = step.url.as_deref().filter(|u| is_loadable(u)) {
                    self.goto(url);
                }
            }
            "goto" => {
                if let Some(url) = step.url.as_deref() {
                    self.goto(url);
                }
            }
            "navigated" => {
                let url = step.url.clone().unwrap_or_default();
                match &self.last {
                    // A click/Enter just loaded a new page: wait for it, do not load it again.
                    // (Long after the last action it was typed in the address bar.)
                    Last::Element { ts_ms, .. } if step.ts_ms <= ts_ms + NAV_AFTER_ACTION_MS => {
                        self.push(
                            format!("await browser.nav.wait({{ for: \"load\" }}); // → {url}"),
                            Last::Other,
                        )
                    }
                    _ if is_loadable(&url) => self.goto(&url),
                    _ => {}
                }
            }
            "back" | "forward" | "reload" => self.push(
                format!("await browser.nav.{}({{}});", step.action),
                Last::Other,
            ),
            "open_tab" => {
                let url = step.url.clone().unwrap_or_default();
                self.push(
                    format!("await browser.tabs.open({{ url: {} }});", js(&url)),
                    Last::Goto(url),
                );
            }
            "wait" => self.wait(step),
            "command" => self.command(step),
            _ if step.is_element_action() => self.element(step),
            other => self.comment(&format!("(skipped unknown step {other})")),
        }
    }

    fn wait(&mut self, step: &Step) {
        let kind = step.wait_for.as_deref().unwrap_or("load");
        match kind {
            "ref" => match step.target.as_ref().filter(|t| t.has_locator()) {
                Some(t) => {
                    let (query, nth) = query_of(t);
                    let nth = if nth > 0 {
                        format!(", {nth}")
                    } else {
                        String::new()
                    };
                    self.push(format!("await locate({query}{nth});"), Last::Other);
                }
                None => self.comment("(skipped a wait for an element that was not identified)"),
            },
            "load" => self.push(
                "await browser.nav.wait({ for: \"load\" });".into(),
                Last::Other,
            ),
            _ => {
                let value = js(step.value.as_deref().unwrap_or_default());
                self.push(
                    format!(
                        "await browser.nav.wait({{ for: {}, value: {value} }});",
                        js(kind)
                    ),
                    Last::Other,
                );
            }
        }
    }

    /// Other recorded commands: read-only ones are dropped, the rest replayed as they were
    /// (without the tab id, which differs between runs).
    fn command(&mut self, step: &Step) {
        let Some(call) = &step.command else { return };
        if api::is_read_only(&call.name) {
            return;
        }
        let mut args = call.args.clone();
        if let Value::Object(map) = &mut args {
            map.remove("tab");
        }
        self.push(
            format!("await browser.call({}, {args});", js(&call.name)),
            Last::Other,
        );
    }

    fn element(&mut self, step: &Step) {
        let action = step.action.as_str();
        let target = step.target.as_ref().filter(|t| t.has_locator());
        let Some(target) = target else {
            match action {
                "press" => {
                    let keys = js(step.value.as_deref().unwrap_or("Enter"));
                    self.push(
                        format!("await browser.input.keys({{ keys: {keys} }});"),
                        Last::Other,
                    );
                }
                "scroll" => self.scroll_comment(step),
                _ => {
                    let css = step
                        .target
                        .as_ref()
                        .and_then(|t| t.css.clone())
                        .unwrap_or_else(|| "?".into());
                    self.comment(&format!(
                        "TODO: {action} on `{css}`: no role/name/text was recorded for it"
                    ));
                }
            }
            return;
        };
        if action == "scroll" && step.source == Source::Human {
            // Actions scroll their element into view themselves.
            self.scroll_comment(step);
            return;
        }
        let key = target_key(target);
        // A double click is recorded as click + dblclick: keep only the dblclick.
        if action == "dblclick"
            && let Last::Element {
                action: last_action,
                key: last_key,
                line,
                ..
            } = &self.last
            && last_action == "click"
            && *last_key == key
        {
            let line = *line;
            self.lines.truncate(line);
        }

        let value = match action {
            "fill" | "type" | "select" => Some(self.value_expr(step)),
            "press" | "scroll" => step.value.as_deref().map(js),
            _ => None,
        };
        let (query, nth) = query_of(target);
        let simple = target.role.is_some() && target.name.is_some() && !is_truncated(target);
        let mut args = if simple {
            format!(
                "{}, {}, {}",
                js(target.role.as_deref().unwrap_or_default()),
                js(target.name.as_deref().unwrap_or_default()),
                js(action)
            )
        } else {
            format!("{query}, {}", js(action))
        };
        match (value, nth) {
            (Some(v), 0) => args.push_str(&format!(", {v}")),
            (Some(v), n) => args.push_str(&format!(", {v}, {n}")),
            (None, 0) => {}
            (None, n) => args.push_str(&format!(", undefined, {n}")),
        }
        let function = if simple { "act" } else { "actBy" };
        let line = self.lines.len();
        self.push(
            format!("await {function}({args});"),
            Last::Element {
                action: action.to_string(),
                key,
                line,
                ts_ms: step.ts_ms,
            },
        );
    }

    fn scroll_comment(&mut self, step: &Step) {
        let y = step
            .extra
            .as_ref()
            .and_then(|e| e["y"].as_i64())
            .unwrap_or(0);
        self.comment(&format!("scrolled the page to y={y}"));
        self.last = Last::Other;
    }
}

/// Names cut by the snapshot end with `…`; they cannot be matched exactly.
fn is_truncated(t: &Target) -> bool {
    t.name.as_deref().is_some_and(|n| n.ends_with('…'))
}

/// A `page.find` query for the target, and its nth.
fn query_of(t: &Target) -> (String, usize) {
    let mut parts = Vec::new();
    if let Some(role) = &t.role {
        parts.push(format!("role: {}", js(role)));
    }
    match (&t.name, &t.text) {
        (Some(name), _) if !is_truncated(t) => parts.push(format!("name: {}", js(name))),
        (Some(name), _) => {
            let prefix = name.trim_end_matches('…').trim_end();
            parts.push(format!("text: {}", js(prefix)));
        }
        (None, Some(text)) => parts.push(format!("text: {}", js(text))),
        (None, None) => {}
    }
    (format!("{{ {} }}", parts.join(", ")), t.nth.unwrap_or(0))
}

fn is_loadable(url: &str) -> bool {
    ["http://", "https://", "file://"]
        .iter()
        .any(|p| url.starts_with(p))
}

/// The steps codegen uses, in the order they happened: no human copies of API actions, no
/// failed calls.
fn replay_steps(steps: &[Step]) -> Vec<&Step> {
    let mut out: Vec<&Step> = steps
        .iter()
        .filter(|s| s.dup_of.is_none() && !s.failed() && !s.pending)
        .collect();
    // Human events carry the page time of the action (a typing burst is reported when it
    // ends): sort by time, keeping the log order for ties.
    out.sort_by_key(|s| (s.ts_ms, s.seq));
    out
}

/// Generate the TypeScript for a recording.
pub fn generate(meta: &RecordingMeta, steps: &[Step], options: Options) -> Generated {
    let mut w = Writer {
        options,
        lines: Vec::new(),
        inputs: Vec::new(),
        input_names: HashMap::new(),
        last: Last::Nothing,
    };
    for step in replay_steps(steps) {
        w.step(step);
    }
    w.lines.push(format!(
        "console.log({});",
        js(&format!("done: {}", meta.name))
    ));

    let mut code = String::new();
    let _ = writeln!(
        code,
        "// Generated by BrowserMCP from recording \"{}\" ({}).",
        meta.name, meta.id
    );
    if !meta.start_url.is_empty() {
        let _ = writeln!(code, "// Recorded on {}", meta.start_url);
    }
    code.push_str("// Types for editors: crates/script/types/browser.d.ts\n//\n");
    if w.inputs.is_empty() {
        code.push_str("// Inputs: none.\n");
    } else {
        code.push_str("// Inputs (--input key=value):\n");
        let width = w.inputs.iter().map(|i| i.name.len()).max().unwrap_or(0);
        for input in &w.inputs {
            let about = match &input.default {
                _ if input.secret => "secret, required".to_string(),
                Some(d) => format!("default {}", js(d)),
                None => "required".to_string(),
            };
            let _ = writeln!(
                code,
                "//   {:width$}  {about} ({})",
                input.name,
                js(&input.label)
            );
        }
    }
    let run_inputs: String = w
        .inputs
        .iter()
        .map(|i| format!(" --input {}=...", i.name))
        .collect();
    let _ = writeln!(
        code,
        "//\n// Run: bmcp-script-host run {}.ts --ws ws://127.0.0.1:47800/v1/ws{run_inputs}\n",
        crate::store::slug(&meta.name)
    );
    code.push_str(HELPERS);
    for line in &w.lines {
        code.push_str(line);
        code.push('\n');
    }
    Generated {
        code,
        inputs: w.inputs,
    }
}

/// One line per step for `record.get format=summary`:
/// `#3 +1.2s api fill textbox "Email" = "ann@example.test"`.
pub fn summary(steps: &[Step]) -> String {
    let start = steps.first().map_or(0, |s| s.ts_ms);
    let mut out = String::new();
    for s in steps {
        let source = match s.source {
            Source::Api => "api",
            Source::Human => "human",
            Source::System => "system",
        };
        let _ = write!(
            out,
            "#{} +{:.1}s {source} {}",
            s.seq,
            s.ts_ms.saturating_sub(start) as f64 / 1000.0,
            s.action
        );
        if let Some(call) = &s.command
            && s.action == "command"
        {
            let _ = write!(out, " {}", call.name);
        }
        if let Some(kind) = &s.wait_for {
            let _ = write!(out, " for={kind}");
        }
        if let Some(t) = &s.target {
            if let Some(role) = &t.role {
                let _ = write!(out, " {role}");
            }
            if let Some(name) = &t.name {
                let _ = write!(out, " {}", js(name));
            } else if let Some(text) = &t.text {
                let _ = write!(out, " text={}", js(text));
            }
            if let Some(nth) = t.nth {
                let _ = write!(out, " #{nth}");
            }
            if !t.has_locator()
                && let Some(r) = &t.ref_
            {
                let _ = write!(out, " {r}");
            }
        }
        if let Some(v) = &s.value {
            let _ = write!(out, " = {}", js(v));
        }
        if let Some(u) = s.url.as_deref().filter(|_| {
            matches!(
                s.action.as_str(),
                "start" | "goto" | "navigated" | "open_tab"
            )
        }) {
            let _ = write!(out, " {u}");
        }
        if s.failed() {
            let error = s.command.as_ref().and_then(|c| c.error.clone());
            let _ = write!(out, " FAILED {}", error.unwrap_or_default());
        }
        if let Some(d) = s.dup_of {
            let _ = write!(out, " (dup of #{d})");
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn golden_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
    }

    fn load(name: &str) -> Vec<Step> {
        let path = golden_dir().join(format!("{name}.jsonl"));
        let text = std::fs::read_to_string(&path).unwrap();
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{path:?}: {e}: {l}")))
            .collect()
    }

    fn meta(name: &str, start_url: &str) -> RecordingMeta {
        RecordingMeta {
            id: format!("{}-20261106-120000", crate::store::slug(name)),
            name: name.into(),
            start_url: start_url.into(),
            ..RecordingMeta::default()
        }
    }

    /// Compare with `tests/golden/<file>`; `UPDATE_GOLDEN=1` rewrites the file instead.
    fn check_golden(file: &str, actual: &str) {
        let path = golden_dir().join(file);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            expected == actual,
            "{file} differs (run with UPDATE_GOLDEN=1 to accept):\n--- expected\n{expected}\n--- actual\n{actual}"
        );
    }

    #[test]
    fn golden_login_api_recording() {
        let steps = load("login");
        let meta = meta("login", "http://127.0.0.1:8000/login.html");
        let plain = generate(&meta, &steps, Options { parametrize: false });
        check_golden("login.ts", &plain.code);
        let param = generate(&meta, &steps, Options { parametrize: true });
        check_golden("login.param.ts", &param.code);
        let names: Vec<&str> = param.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["email", "password"]);
        assert!(param.inputs[1].secret && param.inputs[1].default.is_none());
        // The password never reaches a script, parametrized or not.
        assert!(!plain.code.contains("hunter2") && !param.code.contains("hunter2"));
        check_golden("login.summary.txt", &summary(&steps));
    }

    #[test]
    fn golden_human_recording() {
        let steps = load("human");
        let meta = meta("Search shop", "https://shop.test/");
        let param = generate(&meta, &steps, Options { parametrize: true });
        check_golden("human.param.ts", &param.code);
    }

    #[test]
    fn input_names_come_from_labels() {
        let t = |role: &str, name: &str| Target {
            role: Some(role.into()),
            name: Some(name.into()),
            ..Target::default()
        };
        assert_eq!(
            input_base_name(Some(&t("textbox", "E-mail address")), false),
            "email"
        );
        assert_eq!(
            input_base_name(Some(&t("searchbox", "Find")), false),
            "query"
        );
        assert_eq!(
            input_base_name(Some(&t("textbox", "First name")), false),
            "firstName"
        );
        assert_eq!(
            input_base_name(Some(&t("textbox", "PIN")), true),
            "password"
        );
        assert_eq!(input_base_name(Some(&t("textbox", "Ім'я")), false), "field");
        assert_eq!(
            input_base_name(Some(&t("textbox", "2nd line")), false),
            "field2ndLine"
        );
        assert_eq!(input_base_name(None, false), "field");
    }

    #[test]
    fn same_field_twice_gets_two_inputs_unless_same_value() {
        let fill = |seq: u64, value: &str| Step {
            seq,
            ts_ms: seq,
            action: "fill".into(),
            value: Some(value.into()),
            target: Some(Target {
                role: Some("textbox".into()),
                name: Some("City".into()),
                ..Target::default()
            }),
            ..Step::default()
        };
        let steps = [fill(1, "Kyiv"), fill(2, "Lviv"), fill(3, "Kyiv")];
        let out = generate(&meta("x", ""), &steps, Options { parametrize: true });
        let names: Vec<&str> = out.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["city", "city2"]);
        assert!(
            out.code
                .contains(r#"act("textbox", "City", "fill", inputs.city ?? "Kyiv");"#)
        );
        assert!(out.code.contains(r#"inputs.city2 ?? "Lviv""#));
    }
}
