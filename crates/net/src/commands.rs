//! Capture and inspection commands: `net.capture`, `net.list`, `net.get`, `net.search`,
//! `net.ws_frames`, `net.har_export`, `net.api_to_code`.
//!
//! Token rules (docs/plan.md): one short line per request, pagination with cursors, bodies
//! only when asked for and cut to `max_bytes`.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use bmcp_core::{CmdError, Command, Ctx, DataDir, ErrorCode, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::apicode;
use crate::format::{self, body_text, is_textual};
use crate::har;
use crate::state::{Env, NetState, TabNet};
use crate::store::{Body, Entry, body_from_cdp, cut};

/// Default page size of `net.list`.
const LIST_DEFAULT: usize = 50;
/// Default body budget of `net.get` / `net.replay`.
const BODY_DEFAULT: usize = 4096;
/// `net.har_export` without `path` returns the HAR inline up to this size.
const HAR_INLINE_MAX: usize = 16 * 1024;
/// `net.search in:body` downloads at most this many missing bodies.
const SEARCH_FETCH_MAX: usize = 200;
/// `net.api_to_code` looks at this many response bodies per endpoint.
const SAMPLES_PER_ENDPOINT: usize = 3;

/// The tab's state for read-only commands (does not start anything).
async fn read_tab(ctx: &Ctx, tab: Option<TabId>) -> Result<(TabId, Arc<TabNet>), CmdError> {
    let tab = ctx.resolve_tab(tab).await?;
    Ok((tab, NetState::from_ctx(ctx).tab(tab)))
}

fn not_captured(tab: TabId) -> CmdError {
    CmdError::new(
        ErrorCode::NotFound,
        format!("nothing captured on tab {tab}"),
    )
    .with_hint("Call net_capture {on:true} first, then reload or use the page.")
}

fn no_entry(tab: TabId, id: u64) -> CmdError {
    CmdError::new(
        ErrorCode::NotFound,
        format!("no request #{id} on tab {tab}"),
    )
    .with_retry(bmcp_core::Retry::No)
    .with_hint("Ids come from net_list; the oldest requests are dropped after 5000.")
}

/// A response body: the one in memory, or downloaded now with `Network.getResponseBody`
/// (Chromium keeps bodies for a while after the request finished). Cached in the entry.
async fn ensure_body(env: &Env, id: u64) -> Result<Body, String> {
    let request_id = {
        let inner = env.tn.lock();
        let entry = inner
            .store
            .get(id)
            .ok_or("request dropped from the buffer")?;
        if let Some(body) = &entry.body {
            return Ok(body.clone());
        }
        if entry.resource_type == "websocket" {
            return Err("websockets have frames, not a body (use net_ws_frames)".into());
        }
        // After a redirect the CDP id belongs to the last hop only.
        if inner.store.id_for(&entry.request_id) != Some(id) {
            return Err("redirect response: no body".into());
        }
        if entry.pending() {
            return Err("still loading".into());
        }
        entry.request_id.clone()
    };
    let reply = env
        .cdp(
            "Network.getResponseBody",
            json!({ "requestId": request_id }),
        )
        .await
        .map_err(|e| format!("not available ({})", e.message))?;
    let body = body_from_cdp(&reply);
    env.tn.lock().store.set_body(id, body.clone());
    Ok(body)
}

/// A body as plain text (for searching and type inference); `None` for binary or cut bodies.
fn body_as_text(body: &Body) -> Option<String> {
    if body.truncated {
        return None;
    }
    if body.base64 {
        String::from_utf8(B64.decode(&body.data).ok()?).ok()
    } else {
        Some(body.data.clone())
    }
}

// ------------------------------------------------------------------ net.capture

pub struct Capture;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// true = start recording the tab's traffic, false = stop (what was captured stays).
    pub on: bool,
    /// Also keep every response body in memory as soon as it arrives (default false: bodies
    /// are fetched when net_get asks, which works until the browser evicts them).
    pub bodies: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CaptureOutput {
    pub tab: TabId,
    pub capturing: bool,
    pub bodies: bool,
    /// Requests in the buffer now.
    pub entries: usize,
}

impl Command for Capture {
    const NAME: &'static str = "net.capture";
    type Args = CaptureArgs;
    type Output = CaptureOutput;

    fn meta() -> Meta {
        Meta::new(
            "Start or stop recording a tab's network traffic (requests, responses, timing, \
             WebSocket frames; last 5000 requests). Then use net_list / net_get / net_search.",
            Toolset::Net,
        )
    }

    async fn run(ctx: Ctx, args: CaptureArgs) -> Result<CaptureOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let _config = env.tn.config.lock().await;
        if args.on {
            env.ensure_listener().await?;
            {
                let mut inner = env.tn.lock();
                inner.capture = true;
                inner.bodies = args.bodies.unwrap_or(false);
            }
            // Request bodies up to 64 kB come with `requestWillBeSent`.
            if let Err(err) = env
                .cdp("Network.enable", json!({ "maxPostDataSize": 65536 }))
                .await
            {
                let mut inner = env.tn.lock();
                inner.capture = false;
                inner.stop_listener_if_idle();
                return Err(err);
            }
        } else {
            {
                let mut inner = env.tn.lock();
                inner.capture = false;
                inner.stop_listener_if_idle();
            }
            env.cdp("Network.disable", json!({})).await?;
        }
        let inner = env.tn.lock();
        Ok(CaptureOutput {
            tab,
            capturing: inner.capture,
            bodies: inner.bodies,
            entries: inner.store.len(),
        })
    }
}

// ------------------------------------------------------------------ net.list

pub struct List;

/// A status filter: a code (404), a class ("4xx"), "failed" or "pending".
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum StatusFilter {
    Code(u16),
    Text(String),
}

/// Which requests to show. All given parts must match.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListFilter {
    /// Case-insensitive substring of the URL.
    pub url_contains: Option<String>,
    /// HTTP method, e.g. "POST".
    pub method: Option<String>,
    /// 404, "4xx", "failed" or "pending".
    pub status: Option<StatusFilter>,
    /// Resource type(s), comma-separated: xhr, fetch, document, script, stylesheet, image,
    /// font, media, websocket, other…
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
}

impl ListFilter {
    pub fn matches(&self, e: &Entry) -> bool {
        if let Some(part) = &self.url_contains
            && !e
                .url
                .to_ascii_lowercase()
                .contains(&part.to_ascii_lowercase())
        {
            return false;
        }
        if let Some(m) = &self.method
            && !e.method.eq_ignore_ascii_case(m)
        {
            return false;
        }
        if let Some(types) = &self.resource_type
            && !types
                .split(',')
                .any(|t| t.trim().eq_ignore_ascii_case(&e.resource_type))
        {
            return false;
        }
        match &self.status {
            None => true,
            Some(StatusFilter::Code(code)) => e.status == Some(*code) && e.error.is_none(),
            Some(StatusFilter::Text(text)) => {
                let text = text.trim().to_ascii_lowercase();
                match text.as_str() {
                    "failed" | "error" | "err" => e.error.is_some(),
                    "pending" => e.pending() && e.status.is_none(),
                    _ => match (text.strip_suffix("xx"), e.status) {
                        (Some(class), Some(status)) => {
                            e.error.is_none() && class == (status / 100).to_string()
                        }
                        (None, Some(status)) => text == status.to_string(),
                        _ => false,
                    },
                }
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    pub filter: Option<ListFilter>,
    /// Show requests from this id on (the `next_cursor` of the previous call). Calling again
    /// with the last `next_cursor` shows only requests that came since.
    pub cursor: Option<u64>,
    /// Lines per page (default 50, max 500).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    pub tab: TabId,
    /// Matching requests from `cursor` on (all pages).
    pub total: usize,
    /// One line per request: `#id METHOD status type size time url [rule notes]`.
    pub text: String,
    /// Pass as `cursor` to continue (or to get only newer requests later).
    pub next_cursor: u64,
    /// More matching requests after this page.
    pub more: bool,
}

impl Command for List {
    const NAME: &'static str = "net.list";
    type Args = ListArgs;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "Captured requests, one compact line each (#12 GET 200 xhr 3.4kB 120ms url), oldest \
             first, filterable by url/method/status/type, paginated with cursor. Details of one \
             request: net_get.",
            Toolset::Net,
        )
    }

    async fn run(ctx: Ctx, args: ListArgs) -> Result<ListOutput, CmdError> {
        let (tab, tn) = read_tab(&ctx, args.tab).await?;
        let limit = args.limit.unwrap_or(LIST_DEFAULT).clamp(1, 500);
        let cursor = args.cursor.unwrap_or(0);
        let filter = args.filter.unwrap_or_default();
        let inner = tn.lock();
        if inner.store.is_empty() && !inner.capture {
            return Err(not_captured(tab));
        }
        let matching: Vec<&Entry> = inner
            .store
            .iter()
            .filter(|e| e.id >= cursor && filter.matches(e))
            .collect();
        let shown = &matching[..matching.len().min(limit)];
        let next_cursor = match shown.last() {
            Some(last) => last.id + 1,
            None => cursor.max(inner.store.next_id()),
        };
        let more = matching.len() > shown.len();
        let mut text = format!(
            "tab {tab}: {} captured{}{}, {} match",
            inner.store.len(),
            if inner.capture { "" } else { " (capture off)" },
            if inner.store.dropped > 0 {
                format!(", {} oldest dropped", inner.store.dropped)
            } else {
                String::new()
            },
            matching.len()
        );
        for e in shown {
            text.push('\n');
            text.push_str(&format::line(e));
        }
        if more {
            text.push_str(&format!(
                "\n… {} more: net_list cursor={next_cursor}",
                matching.len() - shown.len()
            ));
        }
        Ok(ListOutput {
            tab,
            total: matching.len(),
            text,
            next_cursor,
            more,
        })
    }
}

// ------------------------------------------------------------------ net.get

pub struct Get;

/// Extra parts of a request to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Headers,
    ReqBody,
    ResBody,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Request id from net_list (the number after #).
    pub id: u64,
    /// What to show besides the summary: headers, req_body, res_body (default [headers]).
    pub parts: Option<Vec<Part>>,
    /// Cut each body (and very long header values) to this many bytes (default 4096).
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetOutput {
    pub tab: TabId,
    pub id: u64,
    /// Summary, then the requested parts.
    pub text: String,
    /// A body was cut to max_bytes.
    pub truncated: bool,
}

impl Command for Get {
    const NAME: &'static str = "net.get";
    type Args = GetArgs;
    type Output = GetOutput;

    fn meta() -> Meta {
        Meta::new(
            "One captured request: status, timing, initiator and the parts you ask for \
             (headers, req_body, res_body; bodies cut to max_bytes, default 4096).",
            Toolset::Net,
        )
        // Headers and bodies may hold cookies and tokens.
        .secret()
    }

    async fn run(ctx: Ctx, args: GetArgs) -> Result<GetOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let parts = args.parts.unwrap_or_else(|| vec![Part::Headers]);
        let max = args.max_bytes.unwrap_or(BODY_DEFAULT).max(16);
        let entry = env
            .tn
            .lock()
            .store
            .get(args.id)
            .cloned()
            .ok_or_else(|| no_entry(tab, args.id))?;

        let mut text = summary(&entry);
        let mut truncated = false;
        if parts.contains(&Part::Headers) {
            for (title, headers) in [
                ("request headers", &entry.req_headers),
                ("response headers", &entry.res_headers),
            ] {
                text.push_str(&format!("\n{title} ({}):", headers.len()));
                for (name, value) in headers {
                    let shown = cut(value, max);
                    truncated |= shown.len() < value.len();
                    text.push_str(&format!(
                        "\n  {name}: {shown}{}",
                        if shown.len() < value.len() { "…" } else { "" }
                    ));
                }
            }
        }
        if parts.contains(&Part::ReqBody) {
            let body = match &entry.req_body {
                Some(body) => Some(body.clone()),
                // Big bodies are not in the event; ask for them.
                None if entry.has_req_body => env
                    .cdp(
                        "Network.getRequestPostData",
                        json!({ "requestId": entry.request_id }),
                    )
                    .await
                    .ok()
                    .and_then(|r| r["postData"].as_str().map(str::to_string)),
                None => None,
            };
            match body {
                Some(body) => {
                    let shown = cut(&body, max);
                    truncated |= shown.len() < body.len();
                    text.push_str(&format!(
                        "\nrequest body ({}{}):\n{shown}",
                        format::size(body.len() as u64),
                        if shown.len() < body.len() {
                            format!(", cut to {max}B")
                        } else {
                            String::new()
                        }
                    ));
                }
                None => text.push_str("\nrequest body: none"),
            }
        }
        if parts.contains(&Part::ResBody) {
            match ensure_body(&env, args.id).await {
                Ok(body) => {
                    let (shown, cut_here) = body_text(&body, &entry.mime_type, max);
                    truncated |= cut_here;
                    text.push_str(&format!(
                        "\nresponse body ({}{}):\n{shown}",
                        format::size(body.full_len as u64),
                        if cut_here {
                            format!(", cut to {max}B")
                        } else {
                            String::new()
                        }
                    ));
                }
                Err(why) => text.push_str(&format!("\nresponse body: {why}")),
            }
        }
        Ok(GetOutput {
            tab,
            id: args.id,
            text,
            truncated,
        })
    }
}

/// First lines of `net.get`.
fn summary(e: &Entry) -> String {
    let mut facts = Vec::new();
    match (&e.error, e.status) {
        (Some(err), _) => facts.push(format!("failed {err}")),
        (None, Some(status)) => facts.push(format!("status {status} {}", e.status_text)),
        (None, None) => facts.push("pending".into()),
    }
    facts.push(e.resource_type.clone());
    if !e.mime_type.is_empty() {
        facts.push(e.mime_type.clone());
    }
    if e.size > 0 {
        facts.push(format::size(e.size));
    }
    if let Some(ms) = e.duration_ms() {
        facts.push(format::duration(ms));
    }
    if !e.protocol.is_empty() {
        facts.push(e.protocol.clone());
    }
    if let Some(ip) = &e.remote_ip {
        facts.push(ip.clone());
    }
    if e.from_cache {
        facts.push("from cache".into());
    }
    let mut text = format!(
        "#{} {} {}\n{}\ninitiator: {}",
        e.id,
        e.method,
        e.url,
        facts.join(" · ").trim(),
        e.initiator
    );
    if !e.notes.is_empty() {
        text.push_str(&format!("\nnotes: {}", e.notes.join("; ")));
    }
    if e.resource_type == "websocket" {
        text.push_str(&format!(
            "\nframes: {} (net_ws_frames id={})",
            e.frames.len() as u64 + e.frames_dropped,
            e.id
        ));
    }
    text
}

// ------------------------------------------------------------------ net.search

pub struct Search;

/// Where `net.search` looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchIn {
    Url,
    Headers,
    /// Request and response bodies (missing response bodies are downloaded).
    Body,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Text to find (case-insensitive).
    pub query: String,
    /// url, headers or body. Default: url + headers + bodies already in memory.
    #[serde(rename = "in")]
    pub within: Option<SearchIn>,
    /// Max matches shown (default 20).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    pub tab: TabId,
    /// Matching requests.
    pub total: usize,
    /// Request line plus where it matched, with a little context.
    pub text: String,
}

impl Command for Search {
    const NAME: &'static str = "net.search";
    type Args = SearchArgs;
    type Output = SearchOutput;

    fn meta() -> Meta {
        Meta::new(
            "Find captured requests whose url, headers or body contain a text (e.g. a token, \
             an id, a product name). Returns request lines with the matching snippet.",
            Toolset::Net,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: SearchArgs) -> Result<SearchOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let query = args.query.to_ascii_lowercase();
        if query.is_empty() {
            return Err(CmdError::invalid_params("query must not be empty"));
        }
        let limit = args.limit.unwrap_or(20).max(1);
        let (url, headers, body) = match args.within {
            None => (true, true, true),
            Some(SearchIn::Url) => (true, false, false),
            Some(SearchIn::Headers) => (false, true, false),
            Some(SearchIn::Body) => (false, false, true),
        };

        // `in: body` downloads missing text bodies first (newest first, bounded).
        if args.within == Some(SearchIn::Body) {
            let missing: Vec<u64> = {
                let inner = env.tn.lock();
                if inner.store.is_empty() && !inner.capture {
                    return Err(not_captured(tab));
                }
                inner
                    .store
                    .iter()
                    .rev()
                    .filter(|e| e.body.is_none() && !e.pending() && e.error.is_none())
                    .filter(|e| is_textual(&e.mime_type) && e.resource_type != "websocket")
                    .take(SEARCH_FETCH_MAX)
                    .map(|e| e.id)
                    .collect()
            };
            for id in missing {
                let _ = ensure_body(&env, id).await;
            }
        }

        let inner = env.tn.lock();
        let mut total = 0;
        let mut text = String::new();
        for e in inner.store.iter() {
            let mut hits = Vec::new();
            if url && let Some(s) = snippet(&e.url, &query) {
                hits.push(format!("url: {s}"));
            }
            if headers {
                for (side, list) in [("req", &e.req_headers), ("res", &e.res_headers)] {
                    for (name, value) in list {
                        let line = format!("{name}: {value}");
                        if let Some(s) = snippet(&line, &query) {
                            hits.push(format!("{side} header {s}"));
                        }
                    }
                }
            }
            if body {
                if let Some(s) = e.req_body.as_deref().and_then(|b| snippet(b, &query)) {
                    hits.push(format!("req body: {s}"));
                }
                if let Some(s) = e
                    .body
                    .as_ref()
                    .and_then(body_as_text)
                    .and_then(|b| snippet(&b, &query))
                {
                    hits.push(format!("res body: {s}"));
                }
                for frame in &e.frames {
                    if let Some(s) = snippet(&frame.data, &query) {
                        hits.push(format!("ws frame: {s}"));
                        break;
                    }
                }
            }
            if hits.is_empty() {
                continue;
            }
            total += 1;
            if total <= limit {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format::line(e));
                for hit in hits.iter().take(3) {
                    text.push_str(&format!("\n  {hit}"));
                }
            }
        }
        if total == 0 {
            text = format!("no request contains {}", json!(args.query));
        } else if total > limit {
            text.push_str(&format!("\n… {} more (raise limit)", total - limit));
        }
        Ok(SearchOutput { tab, total, text })
    }
}

/// `…context MATCH context…` around the first case-insensitive match, on one line.
fn snippet(haystack: &str, query_lower: &str) -> Option<String> {
    // ASCII lowercasing keeps byte positions, so indexes are valid in `haystack` too.
    let pos = haystack.to_ascii_lowercase().find(query_lower)?;
    let mut start = pos.saturating_sub(40);
    while !haystack.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + query_lower.len() + 40).min(haystack.len());
    while !haystack.is_char_boundary(end) {
        end += 1;
    }
    let mut s = haystack[start..end].replace(['\n', '\r'], " ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < haystack.len() {
        s.push('…');
    }
    Some(s)
}

// ------------------------------------------------------------------ net.ws_frames

pub struct WsFrames;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WsFramesArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Id of the websocket request from net_list.
    pub id: u64,
    /// Start at this frame number (the `next_cursor` of the previous call).
    pub cursor: Option<u64>,
    /// Frames per page (default 50).
    pub limit: Option<usize>,
    /// Cut each payload to this many bytes (default 300).
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WsFramesOutput {
    pub tab: TabId,
    pub id: u64,
    /// Frames seen on this socket so far.
    pub total: u64,
    /// One line per frame: `n → text 45B +1.234s payload` (→ sent, ← received).
    pub text: String,
    pub next_cursor: u64,
    pub more: bool,
}

impl Command for WsFrames {
    const NAME: &'static str = "net.ws_frames";
    type Args = WsFramesArgs;
    type Output = WsFramesOutput;

    fn meta() -> Meta {
        Meta::new(
            "Messages of a captured WebSocket (id from net_list), one line each with direction, \
             size, time and payload (cut). Paginated with cursor.",
            Toolset::Net,
        )
    }

    async fn run(ctx: Ctx, args: WsFramesArgs) -> Result<WsFramesOutput, CmdError> {
        let (tab, tn) = read_tab(&ctx, args.tab).await?;
        let limit = args.limit.unwrap_or(50).clamp(1, 500);
        let max = args.max_bytes.unwrap_or(300).max(16);
        let inner = tn.lock();
        let e = inner
            .store
            .get(args.id)
            .ok_or_else(|| no_entry(tab, args.id))?;
        if e.resource_type != "websocket" {
            return Err(CmdError::invalid_params(format!(
                "#{} is a {} request, not a websocket",
                e.id, e.resource_type
            )));
        }
        let total = e.frames.len() as u64 + e.frames_dropped;
        let first = args.cursor.unwrap_or(0).max(e.frames_dropped);
        let mut text = format!(
            "#{} websocket {} — {total} frames{}",
            e.id,
            e.url,
            if e.end_ts.is_some() { ", closed" } else { "" }
        );
        if e.frames_dropped > 0 {
            text.push_str(&format!(" ({} oldest dropped)", e.frames_dropped));
        }
        let skip = (first - e.frames_dropped) as usize;
        let mut next_cursor = first;
        for (i, f) in e.frames.iter().enumerate().skip(skip).take(limit) {
            let n = e.frames_dropped + i as u64;
            let kind = match f.opcode {
                1 => "text",
                2 => "binary",
                8 => "close",
                9 => "ping",
                10 => "pong",
                _ => "frame",
            };
            let payload = cut(&f.data, max);
            text.push_str(&format!(
                "\n{n} {} {kind} {} +{:.3}s {}{}",
                if f.sent { "→" } else { "←" },
                format::size(f.len as u64),
                (f.ts - e.start_ts).max(0.0),
                payload.replace('\n', "\\n"),
                if payload.len() < f.data.len() || f.data.len() < f.len {
                    "…"
                } else {
                    ""
                }
            ));
            next_cursor = n + 1;
        }
        let more = next_cursor < total;
        if more {
            text.push_str(&format!(
                "\n… {} more: net_ws_frames cursor={next_cursor}",
                total - next_cursor
            ));
        }
        Ok(WsFramesOutput {
            tab,
            id: args.id,
            total,
            text,
            next_cursor,
            more,
        })
    }
}

// ------------------------------------------------------------------ net.har_export

pub struct HarExport;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarExportArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// File to write; relative paths go to data/har/. Default: return the HAR inline when
    /// it is small (≤16 kB), else write data/har/tab<N>-<time>.har.
    pub path: Option<String>,
    /// Only export matching requests (same as net_list).
    pub filter: Option<ListFilter>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HarExportOutput {
    pub tab: TabId,
    /// Requests exported.
    pub entries: usize,
    /// Size of the HAR JSON.
    pub bytes: usize,
    /// Where it was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The HAR itself, when small and no path was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub har: Option<Value>,
}

impl Command for HarExport {
    const NAME: &'static str = "net.har_export";
    type Args = HarExportArgs;
    type Output = HarExportOutput;

    fn meta() -> Meta {
        Meta::new(
            "Export captured requests as HAR 1.2 (for DevTools, Burp, k6, …): written to \
             data/har/ or returned inline when small. Bodies are included when in memory.",
            Toolset::Net,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: HarExportArgs) -> Result<HarExportOutput, CmdError> {
        let (tab, tn) = read_tab(&ctx, args.tab).await?;
        let filter = args.filter.unwrap_or_default();
        let (har, entries) = {
            let inner = tn.lock();
            if inner.store.is_empty() && !inner.capture {
                return Err(not_captured(tab));
            }
            let chosen: Vec<&Entry> = inner.store.iter().filter(|e| filter.matches(e)).collect();
            let count = chosen
                .iter()
                .filter(|e| e.resource_type != "websocket")
                .count();
            (har::build(chosen.into_iter()), count)
        };
        let text = serde_json::to_string_pretty(&har)
            .map_err(|e| CmdError::internal(format!("cannot serialize HAR: {e}")))?;
        let bytes = text.len();
        if args.path.is_none() && bytes <= HAR_INLINE_MAX {
            return Ok(HarExportOutput {
                tab,
                entries,
                bytes,
                path: None,
                har: Some(har),
            });
        }
        let dir = har_dir(&ctx);
        let path = match &args.path {
            Some(p) if PathBuf::from(p).is_absolute() => PathBuf::from(p),
            Some(p) => dir.join(p),
            None => dir.join(format!("tab{tab}-{}.har", unix_ms())),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CmdError::internal(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        std::fs::write(&path, text)
            .map_err(|e| CmdError::internal(format!("cannot write {}: {e}", path.display())))?;
        Ok(HarExportOutput {
            tab,
            entries,
            bytes,
            path: Some(path.display().to_string()),
            har: None,
        })
    }
}

/// `data/har`: set explicitly ([`NetState::set_har_dir`]), from a `DataDir` service, from the
/// `--data-dir` switch of this process, or the portable default next to the executable.
fn har_dir(ctx: &Ctx) -> PathBuf {
    if let Some(dir) = NetState::from_ctx(ctx).har_dir() {
        return dir;
    }
    if let Some(data) = ctx.extensions.get::<DataDir>() {
        return data.root().join("har");
    }
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--data-dir=") {
            return PathBuf::from(value).join("har");
        }
        if arg == "--data-dir"
            && let Some(value) = args.get(i + 1)
        {
            return PathBuf::from(value).join("har");
        }
    }
    DataDir::resolve(None)
        .map(|d| d.root().join("har"))
        .unwrap_or_else(|_| PathBuf::from("data/har"))
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ------------------------------------------------------------------ net.api_to_code

pub struct ApiToCode;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiToCodeArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Only requests to hosts containing this, e.g. "api.example.com".
    pub host: Option<String>,
    /// Output language; only "ts" (TypeScript) for now.
    pub lang: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiToCodeOutput {
    pub tab: TabId,
    /// Endpoints found (method + path template).
    pub endpoints: usize,
    /// Captured XHR/fetch requests they came from.
    pub requests: usize,
    /// The generated client.
    pub code: String,
}

impl Command for ApiToCode {
    const NAME: &'static str = "net.api_to_code";
    type Args = ApiToCodeArgs;
    type Output = ApiToCodeOutput;

    fn meta() -> Meta {
        Meta::new(
            "Turn captured XHR/fetch calls into a typed TypeScript client: endpoints clustered \
             by method + path template (ids → {id}), query params and JSON body/response \
             types inferred from the samples.",
            Toolset::Net,
        )
    }

    async fn run(ctx: Ctx, args: ApiToCodeArgs) -> Result<ApiToCodeOutput, CmdError> {
        if let Some(lang) = &args.lang
            && !matches!(lang.to_ascii_lowercase().as_str(), "ts" | "typescript")
        {
            return Err(CmdError::invalid_params(format!(
                "lang {lang:?} is not supported; use \"ts\""
            )));
        }
        let tab = ctx.resolve_tab(args.tab).await?;
        let env = Env::new(&ctx, tab);
        let host = args.host.map(|h| h.to_ascii_lowercase());
        let wanted = |e: &Entry| {
            matches!(e.resource_type.as_str(), "xhr" | "fetch")
                && !e.method.eq_ignore_ascii_case("OPTIONS")
                && host.as_ref().is_none_or(|h| {
                    apicode::split_url(&e.url)
                        .0
                        .to_ascii_lowercase()
                        .contains(h.as_str())
                })
        };

        // Download a few missing response bodies per endpoint (for the response types).
        let missing: Vec<u64> = {
            let inner = env.tn.lock();
            if inner.store.is_empty() && !inner.capture {
                return Err(not_captured(tab));
            }
            let mut per_key: std::collections::HashMap<_, usize> = Default::default();
            let mut missing = Vec::new();
            for e in inner.store.iter().rev().filter(|e| wanted(e)) {
                let ok = e.status.is_some_and(|s| (200..300).contains(&s)) && e.error.is_none();
                let n = per_key.entry(apicode::key(&e.method, &e.url)).or_default();
                if ok && *n < SAMPLES_PER_ENDPOINT {
                    *n += 1;
                    if e.body.is_none() {
                        missing.push(e.id);
                    }
                }
            }
            missing
        };
        for id in missing {
            let _ = ensure_body(&env, id).await;
        }

        let samples: Vec<apicode::Sample> = {
            let inner = env.tn.lock();
            inner
                .store
                .iter()
                .filter(|e| wanted(e))
                .map(|e| apicode::Sample {
                    method: e.method.clone(),
                    url: e.url.clone(),
                    status: e.status,
                    req_body: e.req_body.clone(),
                    res_body: e.body.as_ref().and_then(body_as_text),
                })
                .collect()
        };
        if samples.is_empty() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("no XHR/fetch requests captured on tab {tab}"),
            )
            .with_hint("Capture (net_capture on:true), then use the page so it calls its API."));
        }
        let requests = samples.len();
        let endpoints = apicode::cluster(samples);
        Ok(ApiToCodeOutput {
            tab,
            endpoints: endpoints.len(),
            requests,
            code: apicode::typescript(&endpoints, requests),
        })
    }
}
