//! [`TrafficStore`]: the captured requests of one tab, built from CDP `Network.*` events.
//!
//! This part is plain data + one big `match` over event names, with no async and no I/O, so it
//! is easy to test: feed it events, look at the entries.
//!
//! Every request gets a small number (`#12`) that the commands use as `id`. CDP's own
//! `requestId` is kept too (it is needed for `Network.getResponseBody`). A redirect reuses the
//! CDP `requestId`, so each hop becomes its own entry and the map `requestId → newest entry`
//! is updated.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

/// Default number of requests kept per tab; older ones are dropped first.
pub const DEFAULT_CAPACITY: usize = 5000;
/// Bodies kept in memory (captured with `bodies: true` or fetched on demand) are cut here.
pub const MAX_STORED_BODY: usize = 256 * 1024;
/// WebSocket frames kept per socket; older frames are dropped first.
pub const MAX_FRAMES: usize = 1000;
/// Payload bytes kept per WebSocket frame.
pub const MAX_FRAME_BYTES: usize = 16 * 1024;
/// "Extra info" events that arrive before their request are parked here (bounded).
const MAX_PENDING_EXTRA: usize = 500;

/// Header list in the order the browser reported it. Names keep their original case.
pub type Headers = Vec<(String, String)>;

/// A response (or request) body.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The text, or base64 for binary bodies (see `base64`). Cut to [`MAX_STORED_BODY`].
    pub data: String,
    pub base64: bool,
    /// True when `data` is not the whole body.
    pub truncated: bool,
    /// Size of the whole body in bytes (as far as we know).
    pub full_len: usize,
}

/// One WebSocket message.
#[derive(Debug, Clone, PartialEq)]
pub struct WsFrame {
    /// True = sent by the page, false = received from the server.
    pub sent: bool,
    /// CDP monotonic timestamp (seconds).
    pub ts: f64,
    /// 1 = text, 2 = binary (base64 payload), 8 = close, 9/10 = ping/pong.
    pub opcode: u8,
    /// Payload, cut to [`MAX_FRAME_BYTES`].
    pub data: String,
    /// Full payload length.
    pub len: usize,
}

/// One captured request (or one hop of a redirect chain, or one WebSocket).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// Our number: `#12`. Grows by one per entry and is never reused.
    pub id: u64,
    /// CDP `requestId` (for `Network.getResponseBody`).
    pub request_id: String,
    pub url: String,
    pub method: String,
    /// Lowercase CDP resource type: document, xhr, fetch, script, image, websocket, …
    pub resource_type: String,
    pub req_headers: Headers,
    /// `request.postData` (CDP gives up to `maxPostDataSize` bytes).
    pub req_body: Option<String>,
    /// The request had a body, even if CDP did not include it.
    pub has_req_body: bool,
    /// Who started it, e.g. `script https://a.test/app.js:12` or `parser https://a.test/`.
    pub initiator: String,
    /// Wall clock start, seconds since the Unix epoch.
    pub wall_time: f64,
    /// Monotonic start (seconds), same clock as all other CDP timestamps.
    pub start_ts: f64,
    pub status: Option<u16>,
    pub status_text: String,
    pub mime_type: String,
    pub res_headers: Headers,
    /// `http/1.1`, `h2`, …
    pub protocol: String,
    pub remote_ip: Option<String>,
    /// CDP `ResourceTiming` (for HAR timings).
    pub timing: Option<Value>,
    pub from_cache: bool,
    /// Bytes on the wire (encoded, incl. headers as CDP counts them).
    pub size: u64,
    /// Monotonic end (seconds) once finished or failed.
    pub end_ts: Option<f64>,
    /// `net::ERR_…` when the request failed.
    pub error: Option<String>,
    /// Response body if we have one in memory.
    pub body: Option<Body>,
    /// WebSocket messages (only for `resource_type == "websocket"`).
    pub frames: VecDeque<WsFrame>,
    /// Frames dropped because of [`MAX_FRAMES`]; frame numbers keep counting from here.
    pub frames_dropped: u64,
    /// What rules/breakpoints did to it, e.g. `r1 mock`.
    pub notes: Vec<String>,
    /// The `requestWillBeSentExtraInfo` / `responseReceivedExtraInfo` headers (raw, with
    /// cookies) arrived; they win over the ones from the main events.
    req_extra: bool,
    res_extra: bool,
}

impl Entry {
    /// Duration in milliseconds once finished.
    pub fn duration_ms(&self) -> Option<f64> {
        let end = self.end_ts?;
        if self.start_ts <= 0.0 {
            return None;
        }
        Some(((end - self.start_ts) * 1000.0).max(0.0))
    }

    /// Still waiting for the response or the end of the body.
    pub fn pending(&self) -> bool {
        self.end_ts.is_none() && self.error.is_none()
    }

    /// The first header with this name (case-insensitive).
    pub fn res_header(&self, name: &str) -> Option<&str> {
        header(&self.res_headers, name)
    }
}

/// The first header with this name (case-insensitive).
pub fn header<'a>(headers: &'a Headers, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Ring buffer of entries plus the `requestId` index.
#[derive(Debug)]
pub struct TrafficStore {
    entries: VecDeque<Entry>,
    capacity: usize,
    next_id: u64,
    /// CDP `requestId` → our id of the newest entry with that request id.
    by_request: HashMap<String, u64>,
    /// Extra-info request headers that arrived before `requestWillBeSent`.
    pending_extra: HashMap<String, Headers>,
    /// How many entries were dropped because the buffer was full.
    pub dropped: u64,
}

impl Default for TrafficStore {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl TrafficStore {
    pub fn new(capacity: usize) -> Self {
        TrafficStore {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            next_id: 1,
            by_request: HashMap::new(),
            pending_extra: HashMap::new(),
            dropped: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    /// The entry `#id`, if still in the buffer.
    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.index_of(id).map(|i| &self.entries[i])
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.index_of(id).map(|i| &mut self.entries[i])
    }

    /// The id the next entry will get (a `net.list` cursor that shows only new requests).
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Our id of the newest entry for a CDP `requestId`.
    pub fn id_for(&self, request_id: &str) -> Option<u64> {
        self.by_request.get(request_id).copied()
    }

    /// Ids grow by one per entry and the deque is in id order, so the position is a subtraction.
    fn index_of(&self, id: u64) -> Option<usize> {
        let first = self.entries.front()?.id;
        let index = usize::try_from(id.checked_sub(first)?).ok()?;
        (index < self.entries.len()).then_some(index)
    }

    fn by_request_mut(&mut self, request_id: &str) -> Option<&mut Entry> {
        let id = self.id_for(request_id)?;
        self.get_mut(id)
    }

    /// Add a new entry and return its id; drops the oldest one when full.
    fn push(&mut self, mut entry: Entry) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        entry.id = id;
        if let Some(headers) = self.pending_extra.remove(&entry.request_id) {
            entry.req_headers = headers;
            entry.req_extra = true;
        }
        self.by_request.insert(entry.request_id.clone(), id);
        self.entries.push_back(entry);
        while self.entries.len() > self.capacity {
            if let Some(old) = self.entries.pop_front() {
                self.dropped += 1;
                // Only forget the index if it still points at the dropped entry.
                if self.by_request.get(&old.request_id) == Some(&old.id) {
                    self.by_request.remove(&old.request_id);
                }
            }
        }
        id
    }

    /// Update the store from one CDP event. Unknown events are ignored.
    pub fn apply(&mut self, method: &str, p: &Value) {
        let request_id = p["requestId"].as_str().unwrap_or_default();
        if request_id.is_empty() {
            return;
        }
        match method {
            "Network.requestWillBeSent" => self.request_will_be_sent(request_id, p),
            "Network.requestWillBeSentExtraInfo" => {
                let headers = headers_from(&p["headers"]);
                if let Some(e) = self.by_request_mut(request_id) {
                    e.req_headers = headers;
                    e.req_extra = true;
                } else if self.pending_extra.len() < MAX_PENDING_EXTRA {
                    self.pending_extra.insert(request_id.to_string(), headers);
                }
            }
            "Network.responseReceived" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    if let Some(t) = p["type"].as_str() {
                        e.resource_type = t.to_ascii_lowercase();
                    }
                    apply_response(e, &p["response"]);
                }
            }
            "Network.responseReceivedExtraInfo" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.res_headers = headers_from(&p["headers"]);
                    e.res_extra = true;
                    if e.status.is_none() {
                        e.status = p["statusCode"].as_u64().map(|s| s as u16);
                    }
                }
            }
            "Network.requestServedFromCache" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.from_cache = true;
                }
            }
            "Network.dataReceived" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.size += p["encodedDataLength"].as_u64().unwrap_or(0);
                }
            }
            "Network.loadingFinished" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.end_ts = p["timestamp"].as_f64();
                    if let Some(len) = p["encodedDataLength"].as_u64() {
                        e.size = len;
                    }
                }
            }
            "Network.loadingFailed" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.end_ts = p["timestamp"].as_f64();
                    let mut error = p["errorText"].as_str().unwrap_or("failed").to_string();
                    if p["canceled"].as_bool() == Some(true) {
                        error.push_str(" (canceled)");
                    }
                    if let Some(reason) = p["blockedReason"].as_str() {
                        error.push_str(&format!(" (blocked: {reason})"));
                    }
                    e.error = Some(error);
                }
            }
            "Network.webSocketCreated" => {
                self.push(Entry {
                    request_id: request_id.to_string(),
                    url: str_of(&p["url"]),
                    method: "GET".into(),
                    resource_type: "websocket".into(),
                    initiator: initiator_text(&p["initiator"]),
                    ..Entry::default()
                });
            }
            "Network.webSocketWillSendHandshakeRequest" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.start_ts = p["timestamp"].as_f64().unwrap_or(0.0);
                    e.wall_time = p["wallTime"].as_f64().unwrap_or(0.0);
                    e.req_headers = headers_from(&p["request"]["headers"]);
                }
            }
            "Network.webSocketHandshakeResponseReceived" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    let r = &p["response"];
                    e.status = r["status"].as_u64().map(|s| s as u16);
                    e.status_text = str_of(&r["statusText"]);
                    e.res_headers = headers_from(&r["headers"]);
                }
            }
            "Network.webSocketFrameSent" | "Network.webSocketFrameReceived" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    let r = &p["response"];
                    let payload = r["payloadData"].as_str().unwrap_or_default();
                    let frame = WsFrame {
                        sent: method == "Network.webSocketFrameSent",
                        ts: p["timestamp"].as_f64().unwrap_or(0.0),
                        opcode: r["opcode"].as_u64().unwrap_or(1) as u8,
                        data: cut(payload, MAX_FRAME_BYTES).to_string(),
                        len: payload.len(),
                    };
                    e.size += payload.len() as u64;
                    e.frames.push_back(frame);
                    while e.frames.len() > MAX_FRAMES {
                        e.frames.pop_front();
                        e.frames_dropped += 1;
                    }
                }
            }
            "Network.webSocketFrameError" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.notes
                        .push(format!("ws error: {}", str_of(&p["errorMessage"])));
                }
            }
            "Network.webSocketClosed" => {
                if let Some(e) = self.by_request_mut(request_id) {
                    e.end_ts = p["timestamp"].as_f64();
                }
            }
            _ => {}
        }
    }

    fn request_will_be_sent(&mut self, request_id: &str, p: &Value) {
        // A redirect: the previous hop (same requestId) is finished with the 30x response.
        if !p["redirectResponse"].is_null()
            && let Some(prev) = self.by_request_mut(request_id)
        {
            apply_response(prev, &p["redirectResponse"]);
            prev.end_ts = p["timestamp"].as_f64();
            if let Some(len) = p["redirectResponse"]["encodedDataLength"].as_u64() {
                prev.size = len;
            }
        }
        let r = &p["request"];
        let mut url = str_of(&r["url"]);
        if let Some(fragment) = r["urlFragment"].as_str() {
            url.push_str(fragment);
        }
        self.push(Entry {
            request_id: request_id.to_string(),
            url,
            method: str_of(&r["method"]),
            resource_type: p["type"].as_str().unwrap_or("other").to_ascii_lowercase(),
            req_headers: headers_from(&r["headers"]),
            req_body: r["postData"].as_str().map(str::to_string),
            has_req_body: r["hasPostData"].as_bool().unwrap_or(false) || !r["postData"].is_null(),
            initiator: initiator_text(&p["initiator"]),
            wall_time: p["wallTime"].as_f64().unwrap_or(0.0),
            start_ts: p["timestamp"].as_f64().unwrap_or(0.0),
            ..Entry::default()
        });
    }

    /// Remember a response body for entry `id` (cut to [`MAX_STORED_BODY`]).
    pub fn set_body(&mut self, id: u64, body: Body) {
        if let Some(e) = self.get_mut(id) {
            e.body = Some(body);
        }
    }

    /// Add a note (what a rule did) to the newest entry of a CDP request id.
    pub fn note(&mut self, request_id: &str, note: String) {
        if let Some(e) = self.by_request_mut(request_id)
            && !e.notes.contains(&note)
        {
            e.notes.push(note);
        }
    }
}

/// Build a [`Body`] from `Network.getResponseBody` / `Fetch.getResponseBody` output.
pub fn body_from_cdp(reply: &Value) -> Body {
    let data = reply["body"].as_str().unwrap_or_default();
    let base64 = reply["base64Encoded"].as_bool().unwrap_or(false);
    // Base64 bodies are only useful whole; text bodies are useful even when cut.
    let full_len = if base64 {
        data.len() / 4 * 3
    } else {
        data.len()
    };
    if base64 && data.len() > MAX_STORED_BODY {
        return Body {
            data: String::new(),
            base64,
            truncated: true,
            full_len,
        };
    }
    let kept = cut(data, MAX_STORED_BODY);
    Body {
        data: kept.to_string(),
        base64,
        truncated: kept.len() < data.len(),
        full_len,
    }
}

/// Copy the fields of a CDP `Response` object into the entry.
fn apply_response(e: &mut Entry, r: &Value) {
    e.status = r["status"].as_u64().map(|s| s as u16);
    e.status_text = str_of(&r["statusText"]);
    e.mime_type = str_of(&r["mimeType"]);
    if !e.res_extra {
        e.res_headers = headers_from(&r["headers"]);
    }
    // Request headers as finally sent (with cookies) are here when no extra info came.
    if !e.req_extra && r["requestHeaders"].is_object() {
        e.req_headers = headers_from(&r["requestHeaders"]);
    }
    e.protocol = str_of(&r["protocol"]);
    e.remote_ip = r["remoteIPAddress"]
        .as_str()
        .map(|ip| match r["remotePort"].as_u64() {
            Some(port) => format!("{ip}:{port}"),
            None => ip.to_string(),
        });
    if r["timing"].is_object() {
        e.timing = Some(r["timing"].clone());
    }
    e.from_cache |= r["fromDiskCache"].as_bool().unwrap_or(false)
        || r["fromPrefetchCache"].as_bool().unwrap_or(false);
}

/// CDP `Headers` (a JSON object) → ordered list. Multi-value headers come joined by `\n`.
pub fn headers_from(v: &Value) -> Headers {
    let Some(map) = v.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, value) in map {
        let value = value.as_str().unwrap_or_default();
        for part in value.split('\n') {
            out.push((name.clone(), part.to_string()));
        }
    }
    out
}

/// `{"type":"script","stack":{…}}` → `script https://a/app.js:12`.
fn initiator_text(v: &Value) -> String {
    let kind = v["type"].as_str().unwrap_or("other");
    // For scripts the top stack frame says where the call came from.
    let frame = &v["stack"]["callFrames"][0];
    let (url, line) = if frame.is_object() {
        (frame["url"].as_str(), frame["lineNumber"].as_u64())
    } else {
        (v["url"].as_str(), v["lineNumber"].as_u64())
    };
    match (url, line) {
        (Some(url), Some(line)) if !url.is_empty() => format!("{kind} {url}:{}", line + 1),
        (Some(url), _) if !url.is_empty() => format!("{kind} {url}"),
        _ => kind.to_string(),
    }
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// The longest prefix of `s` with at most `max` bytes that ends on a character boundary.
pub fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
