//! `page.extract {tab?, schema, hint?, cache_key?}`: structured data from a page without an
//! AI call per page.
//!
//! 1. The first call asks the AI for a **plan**: per output field a locator (a CSS selector +
//!    what to read, or a role/name locator run through `page.find`) and post-processing
//!    (regex, type). The AI sees the JSON Schema, a page snapshot, the visible text
//!    (`page.read`) and a compact DOM outline (tags, ids, classes, key attributes).
//! 2. The plan runs deterministically: one `Runtime.evaluate` for all CSS fields, `page.find`
//!    for role fields, then post-processing in Rust.
//! 3. The plan is cached in `data/extract-cache/<cache_key>.json`; later calls (other pages of
//!    the same site, other rows of a run) use it without AI.
//! 4. When required fields come back empty, the AI rebuilds the plan **once**, seeing what
//!    failed; a working new plan replaces the cache.
//!
//! Plan format (also what the AI must answer):
//!
//! ```json
//! {"fields": [
//!   {"field": "title", "css": "h1"},
//!   {"field": "price", "css": ".price", "post": {"regex": "([0-9.,]+)", "type": "number"}},
//!   {"field": "image", "css": "img.main", "attr": "src"},
//!   {"field": "tags", "css": ".tag", "many": true},
//!   {"field": "items", "css": "li.product", "many": true, "fields": [
//!      {"field": "name", "css": ".name"}, {"field": "url", "css": "a", "attr": "href"}]},
//!   {"field": "signed_in", "role": "button", "name": "Sign out", "post": {"type": "boolean"}}
//! ]}
//! ```

use std::path::PathBuf;
use std::sync::LazyLock;

use bmcp_ai::oneshot::TextRequest;
use bmcp_core::{CmdError, Ctx, ErrorCode, TabId};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::HealService;
use crate::patch::json_object;

/// Instructions for building a plan.
pub const SYSTEM: &str = "\
You write data-extraction plans for BrowserMCP. A plan is run later WITHOUT you on this page \
and on other pages of the same kind, so prefer stable selectors: ids, semantic classes, \
itemprop/data-* attributes, element structure; avoid positions like :nth-child unless the \
structure needs them, and avoid text that changes per page.

Reply with ONE JSON object and nothing else: {\"fields\": [ ... ]}, one entry per top-level \
property of the JSON Schema. Each entry:
- \"field\": the property name;
- either \"css\": a CSS selector (document.querySelectorAll) and optional \"attr\": \"text\" \
(default, innerText), \"html\", \"value\", \"href\"/\"src\" (absolute URL) or any attribute name;
- or a role locator: \"role\" plus \"name\" (exact accessible name), \"text\" (substring) or \
\"regex\"; the value is the element's accessible name (from the page snapshot);
- \"many\": true for arrays (all matches); for arrays of objects also give \"fields\": \
[sub-entries with \"css\" relative to each matched element];
- optional \"post\": {\"regex\": \"pattern; group 1 (or the whole match) is kept\", \
\"type\": \"string\"|\"number\"|\"integer\"|\"boolean\", \"default\": value-if-missing}.";

/// Size limits of the page context sent to the AI.
const SNAPSHOT_CHARS: usize = 6_000;
const READ_CHARS: usize = 4_000;
const OUTLINE_CHARS: usize = 14_000;

/// What to post-process a raw value with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Post {
    /// Keep capture group 1 (or the whole match); no match → null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// string, number, integer or boolean (default: the schema's type).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Used when the element is missing or the regex does not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

/// One output field of a plan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldPlan {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub many: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldPlan>>,
    #[serde(default, skip_serializing_if = "is_default_post")]
    pub post: Post,
}

fn is_default_post(post: &Post) -> bool {
    *post == Post::default()
}

/// A selector plan (see the [module docs](self)).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub fields: Vec<FieldPlan>,
}

impl Plan {
    /// Parse and check the AI's answer.
    pub fn parse(text: &str) -> Result<Plan, String> {
        let value = json_object(text).ok_or("the answer was not a JSON object")?;
        let plan: Plan =
            serde_json::from_value(value).map_err(|e| format!("not a valid plan: {e}"))?;
        if plan.fields.is_empty() {
            return Err("the plan has no fields".into());
        }
        plan.check(&plan.fields, false)?;
        Ok(plan)
    }

    fn check(&self, fields: &[FieldPlan], nested: bool) -> Result<(), String> {
        for f in fields {
            let by_role = f.role.is_some() || f.name.is_some() || f.text.is_some();
            match (&f.css, by_role || f.regex.is_some()) {
                (None, false) => return Err(format!("field '{}' has no locator", f.field)),
                (None, true) if nested => {
                    return Err(format!("nested field '{}' needs `css`", f.field));
                }
                (None, true) if f.fields.is_some() => {
                    return Err(format!("field '{}' with `fields` needs `css`", f.field));
                }
                _ => {}
            }
            for pattern in [&f.regex, &f.post.regex].into_iter().flatten() {
                Regex::new(pattern).map_err(|e| format!("field '{}': bad regex: {e}", f.field))?;
            }
            if let Some(sub) = &f.fields {
                self.check(sub, true)?;
            }
        }
        Ok(())
    }

    /// Short stable id: FNV-1a of the plan's JSON.
    pub fn id(&self) -> String {
        format!(
            "{:016x}",
            fnv1a(&serde_json::to_string(self).unwrap_or_default())
        )
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The cache file `data/extract-cache/<key>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub plan_id: String,
    pub cache_key: String,
    /// The page the plan was built on.
    pub url: String,
    pub schema: Value,
    #[serde(default)]
    pub hint: Option<String>,
    pub plan: Plan,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    pub created_ms: u64,
}

/// `[A-Za-z0-9._-]`, at most 100 chars; anything else becomes `_`.
pub fn sanitize_key(key: &str) -> String {
    let key: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let key = key.trim_matches('.').to_string();
    if key.is_empty() { "page".into() } else { key }
}

/// Default key: the page's host plus a hash of the schema (pages of one site that are
/// extracted with the same schema share a plan).
pub fn default_key(url: &str, schema: &Value) -> String {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|h| !h.is_empty())
        .unwrap_or("local");
    sanitize_key(&format!(
        "{host}-{:08x}",
        fnv1a(&schema.to_string()) & 0xffff_ffff
    ))
}

/// The JSON Schema of the property `field` (or `{}`).
fn property<'a>(schema: &'a Value, field: &str) -> &'a Value {
    static EMPTY: Value = Value::Null;
    schema
        .get("properties")
        .and_then(|p| p.get(field))
        .unwrap_or(&EMPTY)
}

/// `"number"` from `{"type": "number"}` or `{"type": ["number", "null"]}`.
fn schema_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(t) => Some(t),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        _ => None,
    }
}

/// `"$1,234.50"` → 1234.5; `"12,5 €"` → 12.5; `"1 000"` → 1000.
pub fn parse_number(text: &str) -> Option<f64> {
    let kept: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
        .collect();
    let normalized = match (kept.contains('.'), kept.matches(',').count()) {
        (true, _) => kept.replace(',', ""),
        // One comma followed by 1-2 digits: a decimal comma.
        (false, 1)
            if kept
                .rsplit(',')
                .next()
                .is_some_and(|d| (1..=2).contains(&d.len())) =>
        {
            kept.replace(',', ".")
        }
        _ => kept.replace(',', ""),
    };
    normalized.parse().ok()
}

/// Apply `post` and the type to one raw value (string or null).
fn post_scalar(raw: &Value, post: &Post, schema: &Value) -> Value {
    let fallback = || post.default.clone().unwrap_or(Value::Null);
    let Some(text) = raw.as_str() else {
        return if raw.is_null() {
            fallback()
        } else {
            raw.clone()
        };
    };
    let mut text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(pattern) = &post.regex {
        let Ok(re) = Regex::new(pattern) else {
            return fallback();
        };
        match re.captures(&text) {
            Some(c) => {
                text = c
                    .get(1)
                    .or_else(|| c.get(0))
                    .map_or("", |m| m.as_str())
                    .to_string()
            }
            None => return fallback(),
        }
    }
    match post.kind.as_deref().or_else(|| schema_type(schema)) {
        Some("number") => parse_number(&text).map_or_else(fallback, |n| json!(n)),
        Some("integer") => parse_number(&text).map_or_else(fallback, |n| json!(n.trunc() as i64)),
        Some("boolean") => {
            let t = text.to_lowercase();
            json!(!(t.is_empty() || ["false", "0", "no", "off"].contains(&t.as_str())))
        }
        _ => json!(text),
    }
}

/// Post-process the raw value of `field` (recursing into arrays and row objects).
fn post_field(raw: &Value, field: &FieldPlan, schema: &Value) -> Value {
    if let Some(sub) = &field.fields {
        // Rows: arrays of objects (or one object).
        let item_schema = if field.many {
            schema.get("items").unwrap_or(&Value::Null)
        } else {
            schema
        };
        let row = |raw: &Value| {
            let mut out = Map::new();
            for f in sub {
                let v = post_field(&raw[&f.field], f, property(item_schema, &f.field));
                out.insert(f.field.clone(), v);
            }
            Value::Object(out)
        };
        return match raw {
            Value::Array(items) => Value::Array(items.iter().map(row).collect()),
            Value::Null => field.post.default.clone().unwrap_or(Value::Null),
            other => row(other),
        };
    }
    match raw {
        Value::Array(items) => {
            let item_schema = schema.get("items").unwrap_or(&Value::Null);
            Value::Array(
                items
                    .iter()
                    .map(|v| post_scalar(v, &field.post, item_schema))
                    .filter(|v| !v.is_null())
                    .collect(),
            )
        }
        other => post_scalar(other, &field.post, schema),
    }
}

/// Required top-level properties that came back empty (null, "", [] or {}).
pub fn missing_required(schema: &Value, data: &Value) -> Vec<String> {
    schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| match &data[*key] {
            Value::Null => true,
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        })
        .map(str::to_string)
        .collect()
}

/// In-page runner of the CSS fields: `PLAN` is replaced by the plan's JSON.
const EXTRACT_JS: &str = r#"(() => { /* __bmcp_extract */
  const plan = PLAN;
  const val = (el, attr) => {
    if (!el) return null;
    if (!attr || attr === "text") return (el.innerText ?? el.textContent ?? "").trim();
    if (attr === "html") return el.innerHTML;
    if (attr === "value") return el.value ?? el.getAttribute("value");
    const v = el.getAttribute(attr);
    if (v != null && (attr === "href" || attr === "src")) {
      try { return new URL(v, document.baseURI).href; } catch { return v; }
    }
    return v;
  };
  const run = (root, fields) => {
    const out = {};
    for (const f of fields) {
      if (!f.css) continue;
      let els = [];
      try { els = Array.from(root.querySelectorAll(f.css)); } catch { out[f.field] = null; continue; }
      if (f.fields) {
        const rows = els.map((el) => run(el, f.fields));
        out[f.field] = f.many ? rows : (rows[0] ?? null);
      } else if (f.many) {
        out[f.field] = els.map((el) => val(el, f.attr));
      } else {
        out[f.field] = val(els[0], f.attr);
      }
    }
    return out;
  };
  return run(document, plan);
})()"#;

/// A compact DOM outline for the AI (tags, ids, classes, key attributes, own text).
const OUTLINE_JS: &str = r##"(() => { /* __bmcp_outline */
  const MAX = LIMIT; let out = "";
  const skip = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "svg", "SVG", "TEMPLATE", "LINK", "META", "IFRAME"]);
  const attrs = ["name", "type", "href", "src", "itemprop", "role", "aria-label", "data-testid", "alt", "title"];
  const walk = (el, depth) => {
    if (out.length > MAX || skip.has(el.tagName)) return;
    let line = "  ".repeat(depth) + "<" + el.tagName.toLowerCase();
    if (el.id) line += "#" + el.id;
    if (typeof el.className === "string" && el.className.trim()) {
      line += "." + el.className.trim().split(/\s+/).slice(0, 4).join(".");
    }
    for (const a of attrs) {
      const v = el.getAttribute(a);
      if (v) line += " " + a + "=\"" + v.slice(0, 60) + "\"";
    }
    line += ">";
    const own = Array.from(el.childNodes).filter((n) => n.nodeType === 3)
      .map((n) => n.textContent.trim()).filter(Boolean).join(" ");
    if (own) line += " " + own.slice(0, 80);
    out += line + "\n";
    for (const c of el.children) walk(c, depth + 1);
  };
  walk(document.body || document.documentElement, 0);
  return out.slice(0, MAX);
})()"##;

/// `- button "Sign in" @3:e7 …` → `Sign in`.
static FIND_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^- \S+ "((?:[^"\\]|\\.)*)""#).unwrap());

/// `page.extract` arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// JSON Schema of the result: an object with `properties` (and `required`).
    pub schema: Value,
    /// Tells the AI what to look for, e.g. "the main product, not the recommendations".
    pub hint: Option<String>,
    /// Name of the cached plan (data/extract-cache/<key>.json); default: the page's host plus
    /// a hash of the schema.
    pub cache_key: Option<String>,
    /// Ignore the cache and build a new plan.
    pub rebuild: Option<bool>,
    /// AI provider for building the plan (default: the configured default provider).
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// `page.extract` result.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExtractOutput {
    /// The extracted object.
    pub data: Value,
    /// True when the cached plan worked (no AI call).
    pub from_cache: bool,
    /// Id of the plan that produced `data`.
    pub plan_id: String,
    pub cache_key: String,
    /// True when the AI built a second plan because the first one missed required fields.
    pub rebuilt: bool,
    /// Required fields still empty (the plan was then not cached).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

/// Why a plan is being rebuilt.
struct Failure<'a> {
    plan: &'a Plan,
    data: &'a Value,
    missing: &'a [String],
}

impl HealService {
    fn cache_path(&self, key: &str) -> PathBuf {
        self.config.extract_cache_dir.join(format!("{key}.json"))
    }

    fn load_cache(&self, key: &str) -> Option<CacheEntry> {
        let text = std::fs::read_to_string(self.cache_path(key)).ok()?;
        serde_json::from_str(&text)
            .inspect_err(|e| tracing::warn!(key, %e, "ignoring a broken extract cache file"))
            .ok()
    }

    fn save_cache(&self, entry: &CacheEntry) -> Result<(), CmdError> {
        let dir = &self.config.extract_cache_dir;
        std::fs::create_dir_all(dir)
            .map_err(|e| CmdError::internal(format!("{}: {e}", dir.display())))?;
        let path = self.cache_path(&entry.cache_key);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(entry).unwrap_or_default();
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| CmdError::internal(format!("{}: {e}", path.display())))
    }

    /// `Runtime.evaluate` in the page, by value.
    async fn evaluate(&self, ctx: &Ctx, tab: TabId, expression: String) -> Result<Value, CmdError> {
        let reply = ctx
            .backend
            .cdp(
                tab,
                "Runtime.evaluate".into(),
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
            )
            .await?;
        if let Some(details) = reply.get("exceptionDetails") {
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("JavaScript exception");
            return Err(CmdError::internal(format!(
                "extraction script failed: {text}"
            )));
        }
        Ok(reply["result"]["value"].clone())
    }

    /// Run `plan` on the page: CSS fields in one evaluate, role fields with page.find.
    async fn run_plan(
        &self,
        ctx: &Ctx,
        tab: TabId,
        plan: &Plan,
        schema: &Value,
    ) -> Result<Value, CmdError> {
        let plan_json = serde_json::to_string(&plan.fields).unwrap_or_else(|_| "[]".into());
        let raw = self
            .evaluate(ctx, tab, EXTRACT_JS.replace("PLAN", &plan_json))
            .await?;
        let mut data = Map::new();
        for field in &plan.fields {
            let value = if field.css.is_some() {
                raw.get(&field.field).cloned().unwrap_or(Value::Null)
            } else {
                self.find_names(ctx, tab, field).await?
            };
            let value = post_field(&value, field, property(schema, &field.field));
            data.insert(field.field.clone(), value);
        }
        Ok(Value::Object(data))
    }

    /// Accessible names of the elements a role locator matches (one, or all with `many`).
    async fn find_names(&self, ctx: &Ctx, tab: TabId, f: &FieldPlan) -> Result<Value, CmdError> {
        let mut args = json!({"tab": tab, "limit": if f.many { 100 } else { 1 }});
        for (key, value) in [
            ("role", &f.role),
            ("name", &f.name),
            ("text", &f.text),
            ("regex", &f.regex),
        ] {
            if let Some(v) = value {
                args[key] = json!(v);
            }
        }
        let found = self.call_as(ctx.caller.clone(), "page.find", args).await?;
        let names: Vec<Value> = found["text"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .filter_map(|l| FIND_NAME.captures(l.trim()))
            .map(|c| json!(c[1].replace("\\\"", "\"")))
            .collect();
        Ok(if f.many {
            Value::Array(names)
        } else {
            names.into_iter().next().unwrap_or(Value::Null)
        })
    }

    /// Ask the AI for a plan for this page (optionally: why the previous one failed).
    async fn build_plan(
        &self,
        ctx: &Ctx,
        tab: TabId,
        url: &str,
        args: &ExtractArgs,
        failure: Option<Failure<'_>>,
    ) -> Result<(Plan, bmcp_ai::oneshot::Completion), CmdError> {
        let caller = ctx.caller.clone();
        let snapshot = self
            .call_as(
                caller.clone(),
                "page.snapshot",
                json!({"tab": tab, "max_chars": SNAPSHOT_CHARS}),
            )
            .await
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        let read = self
            .call_as(
                caller,
                "page.read",
                json!({"tab": tab, "max_chars": READ_CHARS}),
            )
            .await
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        let outline = self
            .evaluate(
                ctx,
                tab,
                OUTLINE_JS.replace("LIMIT", &OUTLINE_CHARS.to_string()),
            )
            .await
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .unwrap_or_default();

        let mut prompt = format!(
            "## JSON Schema of the result\n{}\n\n## Page\n{url}\n",
            serde_json::to_string_pretty(&args.schema).unwrap_or_default()
        );
        if let Some(hint) = &args.hint {
            prompt.push_str(&format!("\n## Hint\n{hint}\n"));
        }
        prompt.push_str(&format!("\n## Accessibility snapshot\n{snapshot}\n"));
        prompt.push_str(&format!("\n## Visible text\n{read}\n"));
        prompt.push_str(&format!(
            "\n## DOM outline (tag#id.class attributes text)\n{outline}\n"
        ));
        if let Some(f) = failure {
            prompt.push_str(&format!(
                "\n## The previous plan missed required fields {:?}\nPlan: {}\nIt extracted: {}\n\
                 Write a better plan.\n",
                f.missing,
                serde_json::to_string(f.plan).unwrap_or_default(),
                bmcp_ai::model::truncate(&f.data.to_string(), 2000)
            ));
        }
        prompt.push_str("\nAnswer with the JSON plan only.");

        let completion = self
            .complete(TextRequest {
                provider: args.provider.clone(),
                model: args.model.clone(),
                system: SYSTEM.to_string(),
                prompt,
                timeout: None,
            })
            .await?;
        let plan = Plan::parse(&completion.text).map_err(|problem| {
            CmdError::internal(format!(
                "the AI did not return a usable extraction plan: {problem}"
            ))
            .with_hint("Try again, give a `hint`, or pick another provider/model.")
        })?;
        Ok((plan, completion))
    }

    /// `page.extract` (see the [module docs](self)).
    pub async fn extract(&self, ctx: &Ctx, args: ExtractArgs) -> Result<ExtractOutput, CmdError> {
        if args
            .schema
            .get("properties")
            .and_then(Value::as_object)
            .is_none()
        {
            return Err(CmdError::invalid_params(
                "schema must be a JSON Schema object with `properties`",
            ));
        }
        let tab = ctx.resolve_tab(args.tab).await?;
        let url = ctx
            .backend
            .list_tabs()
            .await?
            .into_iter()
            .find(|t| t.id == tab)
            .map(|t| t.url)
            .ok_or_else(|| CmdError::new(ErrorCode::TabNotFound, format!("no tab {tab}")))?;
        let key = match &args.cache_key {
            Some(key) => sanitize_key(key),
            None => default_key(&url, &args.schema),
        };

        // 1. The cached plan, else a new one.
        let cached = if args.rebuild == Some(true) {
            None
        } else {
            self.load_cache(&key)
        };
        let (first, first_by) = match cached {
            Some(entry) => (entry.plan, None),
            None => {
                let (plan, completion) = self.build_plan(ctx, tab, &url, &args, None).await?;
                (plan, Some(completion))
            }
        };
        let from_cache = first_by.is_none();
        let data = self.run_plan(ctx, tab, &first, &args.schema).await?;
        let missing = missing_required(&args.schema, &data);
        let entry = |plan: &Plan, by: &bmcp_ai::oneshot::Completion| CacheEntry {
            plan_id: plan.id(),
            cache_key: key.clone(),
            url: url.clone(),
            schema: args.schema.clone(),
            hint: args.hint.clone(),
            plan: plan.clone(),
            provider: by.provider.clone(),
            model: by.model.clone(),
            created_ms: bmcp_jobs::store::now_ms(),
        };
        if missing.is_empty() {
            if let Some(by) = &first_by {
                self.save_cache(&entry(&first, by))?;
            }
            return Ok(ExtractOutput {
                data,
                from_cache,
                plan_id: first.id(),
                cache_key: key,
                rebuilt: false,
                missing,
            });
        }

        // 2. Required fields missing: rebuild once, showing the AI what failed.
        let failure = Failure {
            plan: &first,
            data: &data,
            missing: &missing,
        };
        let (second, by) = self
            .build_plan(ctx, tab, &url, &args, Some(failure))
            .await?;
        let data = self.run_plan(ctx, tab, &second, &args.schema).await?;
        let missing = missing_required(&args.schema, &data);
        if missing.is_empty() {
            self.save_cache(&entry(&second, &by))?;
        }
        Ok(ExtractOutput {
            data,
            from_cache: false,
            plan_id: second.id(),
            cache_key: key,
            rebuilt: true,
            missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_types() {
        assert_eq!(parse_number("$1,234.50"), Some(1234.5));
        assert_eq!(parse_number("12,5 €"), Some(12.5));
        assert_eq!(parse_number("1,000"), Some(1000.0));
        assert_eq!(parse_number("n/a"), None);
        let post = Post {
            regex: Some(r"(\d+) reviews".into()),
            kind: Some("integer".into()),
            default: None,
        };
        assert_eq!(
            post_scalar(&json!("  128 reviews "), &post, &Value::Null),
            json!(128)
        );
        assert_eq!(
            post_scalar(&json!("none"), &post, &Value::Null),
            Value::Null
        );
        let by_schema = post_scalar(
            &json!("In stock"),
            &Post::default(),
            &json!({"type": "boolean"}),
        );
        assert_eq!(by_schema, json!(true));
    }

    #[test]
    fn plans_parse_and_validate() {
        let plan = Plan::parse(
            "```json\n{\"fields\": [{\"field\": \"t\", \"css\": \"h1\"}, \
             {\"field\": \"rows\", \"css\": \"li\", \"many\": true, \"fields\": [{\"field\": \"a\", \"css\": \"a\", \"attr\": \"href\"}]}]}\n```",
        )
        .unwrap();
        assert_eq!(plan.fields.len(), 2);
        assert_eq!(plan.id(), plan.clone().id());
        assert!(Plan::parse("{\"fields\": [{\"field\": \"t\"}]}").is_err());
        assert!(Plan::parse("{\"fields\": []}").is_err());
        assert!(
            Plan::parse(
                "{\"fields\": [{\"field\": \"t\", \"css\": \"h1\", \"post\": {\"regex\": \"(\"}}]}"
            )
            .is_err()
        );
    }

    #[test]
    fn rows_and_required() {
        let field = FieldPlan {
            field: "items".into(),
            css: Some("li".into()),
            many: true,
            fields: Some(vec![FieldPlan {
                field: "price".into(),
                css: Some(".p".into()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let schema =
            json!({"type": "array", "items": {"properties": {"price": {"type": "number"}}}});
        let out = post_field(&json!([{"price": "$5"}, {"price": null}]), &field, &schema);
        assert_eq!(out, json!([{"price": 5.0}, {"price": null}]));

        let schema =
            json!({"properties": {"a": {}, "b": {}, "c": {}}, "required": ["a", "b", "c"]});
        let data = json!({"a": "x", "b": "", "c": []});
        assert_eq!(missing_required(&schema, &data), vec!["b", "c"]);
    }

    #[test]
    fn keys() {
        let schema = json!({"properties": {"a": {}}});
        let key = default_key("https://shop.test/p/1?x=1", &schema);
        assert!(key.starts_with("shop.test-"), "{key}");
        assert_eq!(key, default_key("https://shop.test/p/2", &schema));
        assert_eq!(sanitize_key("../a b"), "_a_b");
        assert_eq!(sanitize_key(""), "page");
    }
}
