//! `net.api_to_code`: turn captured XHR/fetch traffic into a small typed TypeScript client.
//!
//! 1. Cluster requests by method + origin + path template: segments that look like ids
//!    (numbers, UUIDs, long hex/hash strings) become `{id}`, so `/items/1` and `/items/2` are
//!    one endpoint `GET /items/{id}`.
//! 2. For each endpoint infer the query parameters (name, type, optional or not) and the shape
//!    of JSON request/response bodies by merging all samples ([`Shape`]).
//! 3. Print one function per endpoint plus a shared `request()` helper.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

/// One captured request, as far as code generation cares.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub req_body: Option<String>,
    pub res_body: Option<String>,
}

/// One endpoint: all samples with the same [`key`].
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub method: String,
    /// `https://api.test` (scheme + host + port).
    pub origin: String,
    /// `/items/{id}`.
    pub template: String,
    pub samples: Vec<Sample>,
}

/// Cluster key: `(METHOD, origin, path template)`.
pub fn key(method: &str, url: &str) -> (String, String, String) {
    let (origin, path, _) = split_url(url);
    (method.to_ascii_uppercase(), origin, template(&path))
}

/// Group samples into endpoints, sorted by origin, path and method.
pub fn cluster(samples: Vec<Sample>) -> Vec<Endpoint> {
    let mut map: BTreeMap<(String, String, String), Vec<Sample>> = BTreeMap::new();
    for s in samples {
        let (method, origin, template) = key(&s.method, &s.url);
        map.entry((origin, template, method)).or_default().push(s);
    }
    map.into_iter()
        .map(|((origin, template, method), samples)| Endpoint {
            method,
            origin,
            template,
            samples,
        })
        .collect()
}

/// `https://a.test:8080/x/y?q=1#f` → (`https://a.test:8080`, `/x/y`, `q=1`).
pub fn split_url(url: &str) -> (String, String, String) {
    let url = url.split('#').next().unwrap_or_default();
    let (before_query, query) = url.split_once('?').unwrap_or((url, ""));
    let after_scheme = before_query.find("://").map_or(0, |i| i + 3);
    let (origin, path) = match before_query[after_scheme..].find('/') {
        Some(i) => before_query.split_at(after_scheme + i),
        None => (before_query, "/"),
    };
    (origin.to_string(), path.to_string(), query.to_string())
}

/// `/users/42/posts/9f1c…` → `/users/{id}/posts/{id}`.
pub fn template(path: &str) -> String {
    path.split('/')
        .map(|seg| if is_id(seg) { "{id}" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

/// Does a path segment look like an id rather than a fixed name?
fn is_id(seg: &str) -> bool {
    let all = |f: fn(char) -> bool| seg.chars().all(f);
    let has_digit = seg.chars().any(|c| c.is_ascii_digit());
    let has_alpha = seg.chars().any(|c| c.is_ascii_alphabetic());
    !seg.is_empty()
        && (all(|c| c.is_ascii_digit())
            || is_uuid(seg)
            || (seg.len() >= 12 && all(|c| c.is_ascii_hexdigit()) && has_digit)
            || (seg.len() >= 20
                && all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && has_digit
                && has_alpha))
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `a=1&b=x` → [(a, 1), (b, x)] with `%xx` and `+` decoded.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            // `%41` → byte 0x41; a `%` without two hex digits after it stays as it is.
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi * 16 + lo) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ------------------------------------------------------------------ shapes

/// The inferred type of JSON values seen in samples.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// Nothing seen yet (e.g. elements of an empty array).
    Unknown,
    Null,
    Bool,
    Num,
    Str,
    Arr(Box<Shape>),
    /// Fields with how many of the `count` objects had them (fewer = optional).
    Obj {
        fields: BTreeMap<String, (Shape, usize)>,
        count: usize,
    },
    /// Different kinds were seen, e.g. `string | null`.
    Union(Vec<Shape>),
}

/// Deeper levels become `unknown` (keeps the generated code small).
const MAX_DEPTH: usize = 6;
/// Elements per array looked at.
const MAX_ITEMS: usize = 20;

impl Shape {
    pub fn of(v: &Value) -> Shape {
        Self::of_depth(v, 0)
    }

    fn of_depth(v: &Value, depth: usize) -> Shape {
        if depth > MAX_DEPTH {
            return Shape::Unknown;
        }
        match v {
            Value::Null => Shape::Null,
            Value::Bool(_) => Shape::Bool,
            Value::Number(_) => Shape::Num,
            Value::String(_) => Shape::Str,
            Value::Array(items) => Shape::Arr(Box::new(
                items
                    .iter()
                    .take(MAX_ITEMS)
                    .map(|i| Self::of_depth(i, depth + 1))
                    .fold(Shape::Unknown, merge),
            )),
            Value::Object(map) => Shape::Obj {
                fields: map
                    .iter()
                    .map(|(k, v)| (k.clone(), (Self::of_depth(v, depth + 1), 1)))
                    .collect(),
                count: 1,
            },
        }
    }

    /// Same kind of value (both objects, both arrays, …)?
    fn same_kind(&self, other: &Shape) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// TypeScript type text; `indent` = spaces of the current line.
    pub fn ts(&self, indent: usize) -> String {
        match self {
            Shape::Unknown => "unknown".into(),
            Shape::Null => "null".into(),
            Shape::Bool => "boolean".into(),
            Shape::Num => "number".into(),
            Shape::Str => "string".into(),
            Shape::Arr(inner) => match **inner {
                Shape::Union(_) => format!("({})[]", inner.ts(indent)),
                _ => format!("{}[]", inner.ts(indent)),
            },
            Shape::Obj { fields, .. } if fields.is_empty() => "Record<string, unknown>".into(),
            Shape::Obj { fields, count } => {
                let pad = " ".repeat(indent + 2);
                let mut out = String::from("{\n");
                for (name, (shape, seen)) in fields {
                    let optional = if seen < count { "?" } else { "" };
                    out.push_str(&format!(
                        "{pad}{}{optional}: {};\n",
                        ts_key(name),
                        shape.ts(indent + 2)
                    ));
                }
                out.push_str(&" ".repeat(indent));
                out.push('}');
                out
            }
            Shape::Union(list) => list
                .iter()
                .map(|s| s.ts(indent))
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }
}

/// Combine what two samples showed into one shape.
pub fn merge(a: Shape, b: Shape) -> Shape {
    match (a, b) {
        (Shape::Unknown, x) | (x, Shape::Unknown) => x,
        (Shape::Arr(a), Shape::Arr(b)) => Shape::Arr(Box::new(merge(*a, *b))),
        (
            Shape::Obj {
                fields: mut fa,
                count: ca,
            },
            Shape::Obj {
                fields: fb,
                count: cb,
            },
        ) => {
            for (name, (shape_b, seen_b)) in fb {
                let merged = match fa.remove(&name) {
                    Some((shape_a, seen_a)) => (merge(shape_a, shape_b), seen_a + seen_b),
                    None => (shape_b, seen_b),
                };
                fa.insert(name, merged);
            }
            Shape::Obj {
                fields: fa,
                count: ca + cb,
            }
        }
        (Shape::Union(list), x) | (x, Shape::Union(list)) => {
            let mut list = list;
            add_to_union(&mut list, x);
            Shape::Union(list)
        }
        (a, b) if a.same_kind(&b) => a,
        (a, b) => Shape::Union(vec![a, b]),
    }
}

fn add_to_union(list: &mut Vec<Shape>, x: Shape) {
    if let Shape::Union(items) = x {
        for item in items {
            add_to_union(list, item);
        }
        return;
    }
    match list.iter().position(|s| s.same_kind(&x)) {
        Some(i) => {
            let old = std::mem::replace(&mut list[i], Shape::Unknown);
            list[i] = merge(old, x);
        }
        None => list.push(x),
    }
}

/// Object key as TypeScript writes it: bare when it is an identifier, else quoted.
fn ts_key(name: &str) -> String {
    let mut chars = name.chars();
    let ident = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ident {
        name.to_string()
    } else {
        Value::String(name.to_string()).to_string()
    }
}

// ------------------------------------------------------------------ code generation

/// Type of a query parameter from its values: number, boolean or string.
fn param_type(values: &[String]) -> &'static str {
    if !values.is_empty() && values.iter().all(|v| v.parse::<f64>().is_ok()) {
        "number"
    } else if !values.is_empty() && values.iter().all(|v| v == "true" || v == "false") {
        "boolean"
    } else {
        "string"
    }
}

/// `get` + `/api/items/{id}` → `getApiItemsById`.
fn function_name(method: &str, template: &str) -> String {
    let mut name = method.to_ascii_lowercase();
    let mut any = false;
    for seg in template.split('/').filter(|s| !s.is_empty()) {
        any = true;
        if seg == "{id}" {
            name.push_str("ById");
            continue;
        }
        for word in seg.split(|c: char| !c.is_ascii_alphanumeric()) {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                name.push(first.to_ascii_uppercase());
                name.push_str(chars.as_str());
            }
        }
    }
    if !any {
        name.push_str("Root");
    }
    name
}

/// Inferred JSON shape of a set of optional bodies (`None` when none of them is JSON).
fn json_shape<'a>(bodies: impl Iterator<Item = Option<&'a String>>) -> Option<Shape> {
    let shapes: Vec<Shape> = bodies
        .flatten()
        .filter_map(|b| serde_json::from_str::<Value>(b).ok())
        .map(|v| Shape::of(&v))
        .collect();
    if shapes.is_empty() {
        None
    } else {
        Some(shapes.into_iter().fold(Shape::Unknown, merge))
    }
}

const HELPER: &str = r#"type Query = Record<string, string | number | boolean | undefined>;

async function request<T>(
  method: string,
  path: string,
  opts: { query?: Query; json?: unknown; text?: string; raw?: boolean } = {},
): Promise<T> {
  const url = new URL(path, BASE_URL);
  for (const [key, value] of Object.entries(opts.query ?? {})) {
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  const headers: Record<string, string> = {};
  let body: string | undefined;
  if (opts.json !== undefined) {
    headers["content-type"] = "application/json";
    body = JSON.stringify(opts.json);
  } else if (opts.text !== undefined) {
    body = opts.text;
  }
  const res = await fetch(url, { method, headers, body, credentials: "include" });
  if (!res.ok) throw new Error(`${method} ${url.pathname}: HTTP ${res.status}`);
  return (opts.raw ? await res.text() : await res.json()) as T;
}
"#;

/// The whole TypeScript client. `total` = number of captured requests looked at.
pub fn typescript(endpoints: &[Endpoint], total: usize) -> String {
    // The most common origin becomes BASE_URL; other origins get absolute paths.
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in endpoints {
        *counts.entry(e.origin.as_str()).or_default() += e.samples.len();
    }
    let base = counts
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(o, _)| *o)
        .unwrap_or("");

    let mut out = format!(
        "// Generated by BrowserMCP net.api_to_code: {} endpoints from {} captured requests.\n\
         // Types are inferred from the captured samples only; check them before relying on them.\n\n\
         const BASE_URL = {};\n\n{HELPER}",
        endpoints.len(),
        total,
        Value::String(base.to_string())
    );

    let mut used = HashSet::new();
    for e in endpoints {
        let mut name = function_name(&e.method, &e.template);
        let mut n = 2;
        while !used.insert(name.clone()) {
            name = format!("{}{n}", function_name(&e.method, &e.template));
            n += 1;
        }
        out.push('\n');
        out.push_str(&endpoint_code(e, &name, base));
    }
    out
}

fn endpoint_code(e: &Endpoint, name: &str, base: &str) -> String {
    let type_base = {
        let mut chars = name.chars();
        chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
            .unwrap_or_default()
    };
    let mut statuses: Vec<String> = e
        .samples
        .iter()
        .filter_map(|s| s.status)
        .map(|s| s.to_string())
        .collect();
    statuses.sort();
    statuses.dedup();
    let mut out = format!(
        "/** {} {}{} (seen {}×, status {}) */\n",
        e.method,
        if e.origin == base { "" } else { &e.origin },
        e.template,
        e.samples.len(),
        if statuses.is_empty() {
            "?".to_string()
        } else {
            statuses.join("/")
        }
    );

    // Path parameters: one per `{id}`.
    let id_count = e.template.matches("{id}").count();
    let ids: Vec<String> = (1..=id_count)
        .map(|i| {
            if i == 1 {
                "id".to_string()
            } else {
                format!("id{i}")
            }
        })
        .collect();
    let mut path = String::new();
    let mut next_id = ids.iter();
    for (i, seg) in e.template.split('/').enumerate() {
        if i > 0 {
            path.push('/');
        }
        match seg {
            "{id}" => {
                let id = next_id.next().map(String::as_str).unwrap_or("id");
                path.push_str(&format!("${{encodeURIComponent(String({id}))}}"));
            }
            other => path.push_str(&other.replace('`', "\\`").replace("${", "\\${")),
        }
    }
    if e.origin != base {
        path = format!("{}{path}", e.origin);
    }

    // Query parameters: union over samples; optional when not in every sample.
    let mut query: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for s in &e.samples {
        let (_, _, q) = split_url(&s.url);
        let mut names = HashSet::new();
        for (k, v) in parse_query(&q) {
            query.entry(k.clone()).or_default().push(v);
            names.insert(k);
        }
        for k in names {
            *seen.entry(k).or_default() += 1;
        }
    }

    let req_shape = json_shape(e.samples.iter().map(|s| s.req_body.as_ref()));
    let has_text_body = req_shape.is_none() && e.samples.iter().any(|s| s.req_body.is_some());
    let res_shape = json_shape(e.samples.iter().map(|s| s.res_body.as_ref()));
    let any_res_body = e.samples.iter().any(|s| s.res_body.is_some());

    let mut params: Vec<String> = ids
        .iter()
        .map(|id| format!("{id}: string | number"))
        .collect();
    let mut opts: Vec<String> = Vec::new();
    if let Some(shape) = &req_shape {
        out.push_str(&format!("export type {type_base}Body = {};\n", shape.ts(0)));
        params.push(format!("body: {type_base}Body"));
        opts.push("json: body".into());
    } else if has_text_body {
        params.push("body: string".into());
        opts.push("text: body".into());
    }
    if !query.is_empty() {
        let all_optional = query.keys().all(|k| seen[k] < e.samples.len());
        let fields: Vec<String> = query
            .iter()
            .map(|(k, values)| {
                let optional = if seen[k] < e.samples.len() { "?" } else { "" };
                format!("{}{optional}: {}", ts_key(k), param_type(values))
            })
            .collect();
        let default = if all_optional { " = {}" } else { "" };
        params.push(format!("query: {{ {} }}{default}", fields.join("; ")));
        opts.push("query".into());
    }
    let result = match &res_shape {
        Some(shape) => {
            out.push_str(&format!(
                "export type {type_base}Response = {};\n",
                shape.ts(0)
            ));
            format!("{type_base}Response")
        }
        None if any_res_body => {
            opts.push("raw: true".into());
            "string".into()
        }
        // No body seen (not captured, or empty): leave it to the caller.
        None => "unknown".into(),
    };
    let opts = if opts.is_empty() {
        String::new()
    } else {
        format!(", {{ {} }}", opts.join(", "))
    };
    out.push_str(&format!(
        "export function {name}({}): Promise<{result}> {{\n  return request(\"{}\", `{path}`{opts});\n}}\n",
        params.join(", "),
        e.method,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn templates_replace_ids() {
        assert_eq!(template("/api/items/42"), "/api/items/{id}");
        assert_eq!(
            template("/u/3f2b8c1e-1d2a-4b3c-9d8e-0123456789ab/x"),
            "/u/{id}/x"
        );
        assert_eq!(template("/blob/9f1c2b3a4d5e6f70"), "/blob/{id}");
        assert_eq!(template("/api/v2/items"), "/api/v2/items");
        assert_eq!(
            split_url("http://h.test:8/a/b?x=1#f"),
            ("http://h.test:8".into(), "/a/b".into(), "x=1".into())
        );
        assert_eq!(
            split_url("http://h.test"),
            ("http://h.test".into(), "/".into(), "".into())
        );
    }

    #[test]
    fn query_decoding() {
        assert_eq!(
            parse_query("q=a+b%21&page=2&flag"),
            vec![
                ("q".into(), "a b!".into()),
                ("page".into(), "2".into()),
                ("flag".into(), "".into())
            ]
        );
        assert_eq!(parse_query("bad=%zz%"), vec![("bad".into(), "%zz%".into())]);
    }

    #[test]
    fn shapes_merge_into_optional_fields_and_unions() {
        let a = Shape::of(&json!({"id": 1, "name": "x", "tags": ["a"]}));
        let b = Shape::of(&json!({"id": 2, "name": null, "tags": [], "extra": true}));
        let ts = merge(a, b).ts(0);
        assert_eq!(
            ts,
            "{\n  extra?: boolean;\n  id: number;\n  name: string | null;\n  tags: string[];\n}"
        );
        let mixed = Shape::of(&json!([1, "a", {"k-1": 1}]));
        assert_eq!(
            mixed.ts(0),
            "(number | string | {\n  \"k-1\": number;\n})[]"
        );
    }
}
