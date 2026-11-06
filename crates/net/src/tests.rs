//! Command tests against `FakeBackend`: scripted CDP events in, store/list/HAR/Fetch calls out.
//!
//! The listener runs as a tokio task, so after emitting events a test calls [`settle`] to let
//! it (and the tasks it spawns) run. FakeBackend answers instantly, so a few yields suffice.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use bmcp_cdpshare::CdpShare;
use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{BrowserBackend, Caller, CmdError, Ctx, ErrorCode, Registry, TabId};
use serde_json::{Value, json};

use crate::NetState;
use crate::store::{Entry, TrafficStore};

const TAB: TabId = TabId(1);

struct Setup {
    fake: Arc<FakeBackend>,
    registry: Registry,
    ctx: Ctx,
}

impl Setup {
    /// A tab at https://a.test/ whose CDP answers `{}` to the domain/Fetch calls and serves
    /// response bodies from `bodies` (CDP requestId → text).
    fn new(bodies: HashMap<String, String>) -> Setup {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        fake.on_cdp(move |_, method, params| match method {
            "Network.enable"
            | "Network.disable"
            | "Fetch.enable"
            | "Fetch.disable"
            | "Fetch.continueRequest"
            | "Fetch.continueResponse"
            | "Fetch.fulfillRequest"
            | "Fetch.failRequest" => Some(Ok(json!({}))),
            "Network.getResponseBody" | "Fetch.getResponseBody" => {
                let id = params["requestId"].as_str().unwrap_or_default();
                Some(match bodies.get(id) {
                    Some(body) => Ok(json!({ "body": body, "base64Encoded": false })),
                    None => Err(bmcp_core::BackendError::Cdp(
                        "No resource with given identifier found".into(),
                    )),
                })
            }
            _ => None,
        });
        let mut registry = Registry::new();
        crate::register(&mut registry);
        let ctx = ctx(Caller::Rest, fake.clone());
        Setup {
            fake,
            registry,
            ctx,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry.call(self.ctx.clone(), name, args).await
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    fn emit(&self, method: &str, params: Value) {
        self.fake.emit_cdp_event(TAB, method, params);
    }

    /// CDP calls with this method, in order (params only).
    fn calls(&self, method: &str) -> Vec<Value> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p)
            .collect()
    }

    fn net(&self) -> Arc<NetState> {
        self.registry.extensions().get::<NetState>().unwrap()
    }

    /// A finished request: requestWillBeSent + responseReceived + loadingFinished.
    #[allow(clippy::too_many_arguments)]
    fn request(
        &self,
        id: &str,
        method: &str,
        url: &str,
        kind: &str,
        status: u16,
        mime: &str,
        start: f64,
        ms: f64,
        size: u64,
    ) {
        self.emit(
            "Network.requestWillBeSent",
            json!({
                "requestId": id,
                "request": { "url": url, "method": method,
                             "headers": { "Accept": "*/*", "Authorization": "Bearer secret-token" } },
                "timestamp": start,
                "wallTime": 1_709_210_096.0 + start,
                "type": kind,
                "initiator": { "type": "script",
                               "stack": { "callFrames": [{ "url": "https://a.test/app.js", "lineNumber": 11 }] } },
            }),
        );
        self.respond(id, url, kind, status, mime, start + ms / 1000.0, size);
    }

    /// responseReceived + loadingFinished for a request already sent.
    #[allow(clippy::too_many_arguments)]
    fn respond(
        &self,
        id: &str,
        url: &str,
        kind: &str,
        status: u16,
        mime: &str,
        end: f64,
        size: u64,
    ) {
        self.emit(
            "Network.responseReceived",
            json!({
                "requestId": id,
                "type": kind,
                "response": { "url": url, "status": status, "statusText": "OK", "mimeType": mime,
                              "headers": { "Content-Type": mime }, "protocol": "h2",
                              "remoteIPAddress": "10.0.0.1", "remotePort": 443 },
            }),
        );
        self.emit(
            "Network.loadingFinished",
            json!({ "requestId": id, "timestamp": end, "encodedDataLength": size }),
        );
    }
}

/// Let the listener and the tasks it spawned run.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

fn paused_event(id: &str, method: &str, url: &str) -> Value {
    json!({
        "requestId": id,
        "networkId": format!("net-{id}"),
        "resourceType": "XHR",
        "request": { "url": url, "method": method,
                     "headers": { "Accept": "*/*", "Origin": "https://a.test" } },
    })
}

fn response_paused(id: &str, url: &str, status: u16) -> Value {
    let mut p = paused_event(id, "GET", url);
    p["responseStatusCode"] = json!(status);
    p["responseStatusText"] = json!("OK");
    p["responseHeaders"] = json!([
        { "name": "Content-Type", "value": "application/json" },
        { "name": "Content-Length", "value": "15" },
    ]);
    p
}

// ------------------------------------------------------------------ capture, list, get

#[tokio::test]
async fn capture_lists_requests_compactly_with_filters_and_cursor() {
    let s = Setup::new(HashMap::new());
    let out = s.ok("net.capture", json!({ "on": true })).await;
    assert_eq!(out["capturing"], true);
    assert_eq!(
        s.calls("Network.enable"),
        vec![json!({ "maxPostDataSize": 65536 })]
    );

    s.request(
        "1",
        "GET",
        "https://a.test/",
        "Document",
        200,
        "text/html",
        10.0,
        50.0,
        2100,
    );
    s.request(
        "2",
        "GET",
        "https://api.a.test/items?page=2",
        "XHR",
        200,
        "application/json",
        10.1,
        120.0,
        3400,
    );
    s.request(
        "3",
        "POST",
        "https://api.a.test/items",
        "Fetch",
        404,
        "application/json",
        10.2,
        30.0,
        512,
    );
    s.emit(
        "Network.requestWillBeSent",
        json!({ "requestId": "4", "request": { "url": "https://ads.test/x.js", "method": "GET", "headers": {} },
                "timestamp": 10.3, "wallTime": 1.0, "type": "Script", "initiator": { "type": "parser", "url": "https://a.test/" } }),
    );
    s.emit(
        "Network.loadingFailed",
        json!({ "requestId": "4", "timestamp": 10.31, "errorText": "net::ERR_BLOCKED_BY_CLIENT" }),
    );
    s.emit(
        "Network.requestWillBeSent",
        json!({ "requestId": "5", "request": { "url": "https://a.test/slow", "method": "GET", "headers": {} },
                "timestamp": 10.4, "wallTime": 1.0, "type": "Fetch", "initiator": { "type": "other" } }),
    );
    settle().await;

    let out = s.ok("net.list", json!({})).await;
    assert_eq!(
        out["text"],
        "tab 1: 5 captured, 5 match\n\
         #1 GET 200 document 2.1kB 50ms https://a.test/\n\
         #2 GET 200 xhr 3.4kB 120ms https://api.a.test/items?page=2\n\
         #3 POST 404 fetch 512B 30ms https://api.a.test/items\n\
         #4 GET ERR script - 10ms https://ads.test/x.js !net::ERR_BLOCKED_BY_CLIENT\n\
         #5 GET … fetch - - https://a.test/slow"
    );
    assert_eq!(out["next_cursor"], 6);
    assert_eq!(out["more"], false);

    let only = |filter: Value| {
        let s = &s;
        async move {
            let out = s.ok("net.list", json!({ "filter": filter })).await;
            out["text"]
                .as_str()
                .unwrap()
                .lines()
                .skip(1)
                .map(|l| l.split(' ').next().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(only(json!({ "status": "4xx" })).await, ["#3"]);
    assert_eq!(only(json!({ "status": 200 })).await, ["#1", "#2"]);
    assert_eq!(only(json!({ "status": "failed" })).await, ["#4"]);
    assert_eq!(only(json!({ "status": "pending" })).await, ["#5"]);
    assert_eq!(
        only(json!({ "type": "xhr,fetch" })).await,
        ["#2", "#3", "#5"]
    );
    assert_eq!(only(json!({ "method": "post" })).await, ["#3"]);
    assert_eq!(only(json!({ "url_contains": "API.A" })).await, ["#2", "#3"]);

    // Pages of 2, then "only new ones" polling with the last cursor.
    let page = s.ok("net.list", json!({ "limit": 2 })).await;
    assert_eq!(page["more"], true);
    assert!(
        page["text"]
            .as_str()
            .unwrap()
            .ends_with("… 3 more: net_list cursor=3")
    );
    let page = s.ok("net.list", json!({ "limit": 2, "cursor": 3 })).await;
    assert!(page["text"].as_str().unwrap().contains("#3 POST"));
    let page = s.ok("net.list", json!({ "cursor": 6 })).await;
    assert_eq!(page["total"], 0);
    assert_eq!(page["next_cursor"], 6);

    // Capture off: Network.disable, entries stay.
    let out = s.ok("net.capture", json!({ "on": false })).await;
    assert_eq!(out["entries"], 5);
    assert_eq!(s.calls("Network.disable").len(), 1);
    s.request(
        "9",
        "GET",
        "https://a.test/late",
        "XHR",
        200,
        "text/plain",
        20.0,
        1.0,
        1,
    );
    settle().await;
    let out = s.ok("net.list", json!({})).await;
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .starts_with("tab 1: 5 captured (capture off)")
    );
}

#[tokio::test]
async fn list_without_capture_says_what_to_do() {
    let s = Setup::new(HashMap::new());
    let err = s.call("net.list", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.hint.unwrap().contains("net_capture"));
}

#[tokio::test]
async fn get_shows_headers_and_fetches_bodies_lazily_with_a_cap() {
    let body = format!("{{\"items\":[{}]}}", "1,".repeat(100) + "1");
    let s = Setup::new(HashMap::from([("2".to_string(), body.clone())]));
    s.ok("net.capture", json!({ "on": true })).await;
    s.request(
        "2",
        "GET",
        "https://api.a.test/items?page=2",
        "XHR",
        200,
        "application/json",
        1.0,
        120.0,
        3400,
    );
    settle().await;

    let out = s.ok("net.get", json!({ "id": 1 })).await;
    let text = out["text"].as_str().unwrap();
    assert!(
        text.starts_with(
            "#1 GET https://api.a.test/items?page=2\n\
         status 200 OK · xhr · application/json · 3.4kB · 120ms · h2 · 10.0.0.1:443\n\
         initiator: script https://a.test/app.js:12\n\
         request headers (2):\n  Accept: */*\n  Authorization: Bearer secret-token\n\
         response headers (1):\n  Content-Type: application/json"
        ),
        "{text}"
    );
    assert!(
        s.calls("Network.getResponseBody").is_empty(),
        "no body unless asked"
    );

    let out = s
        .ok(
            "net.get",
            json!({ "id": 1, "parts": ["res_body"], "max_bytes": 20 }),
        )
        .await;
    let text = out["text"].as_str().unwrap();
    assert!(
        text.ends_with("response body (213B, cut to 20B):\n{\"items\":[1,1,1,1,1,"),
        "{text}"
    );
    assert_eq!(out["truncated"], true);
    assert_eq!(
        s.calls("Network.getResponseBody"),
        vec![json!({ "requestId": "2" })]
    );
    // The body is cached: asking again does not call CDP again.
    s.ok("net.get", json!({ "id": 1, "parts": ["res_body"] }))
        .await;
    assert_eq!(s.calls("Network.getResponseBody").len(), 1);

    let err = s.call("net.get", json!({ "id": 99 })).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn bodies_true_downloads_each_body_when_finished() {
    let s = Setup::new(HashMap::from([("7".to_string(), "hello".to_string())]));
    s.ok("net.capture", json!({ "on": true, "bodies": true }))
        .await;
    s.request(
        "7",
        "GET",
        "https://a.test/t.txt",
        "Fetch",
        200,
        "text/plain",
        1.0,
        5.0,
        5,
    );
    settle().await;
    assert_eq!(s.calls("Network.getResponseBody").len(), 1);
    let tn = s.net().tab(TAB);
    assert_eq!(
        tn.lock().store.get(1).unwrap().body.as_ref().unwrap().data,
        "hello"
    );
}

#[test]
fn redirects_become_one_entry_per_hop() {
    // The redirect's requestWillBeSent reuses the requestId and carries the 30x response.
    let mut store = TrafficStore::new(10);
    store.apply(
        "Network.requestWillBeSent",
        &json!({ "requestId": "r", "request": { "url": "http://a.test/old", "method": "GET" }, "timestamp": 1.0 }),
    );
    store.apply(
        "Network.requestWillBeSent",
        &json!({ "requestId": "r", "request": { "url": "https://a.test/new", "method": "GET" }, "timestamp": 1.2,
                 "redirectResponse": { "status": 301, "statusText": "Moved", "headers": { "Location": "https://a.test/new" } } }),
    );
    store.apply(
        "Network.responseReceived",
        &json!({ "requestId": "r", "response": { "status": 200, "headers": {} } }),
    );
    let hops: Vec<&Entry> = store.iter().collect();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].status, Some(301));
    assert_eq!(hops[0].duration_ms().map(|ms| ms.round()), Some(200.0));
    assert_eq!(hops[0].res_header("location"), Some("https://a.test/new"));
    assert_eq!(hops[1].status, Some(200));
    assert_eq!(store.id_for("r"), Some(2));
}

#[test]
fn extra_info_headers_win_even_when_they_come_first() {
    let mut store = TrafficStore::new(10);
    store.apply(
        "Network.requestWillBeSentExtraInfo",
        &json!({ "requestId": "x", "headers": { "Cookie": "sid=1", "Accept": "*/*" } }),
    );
    store.apply(
        "Network.requestWillBeSent",
        &json!({ "requestId": "x", "request": { "url": "https://a.test/", "method": "GET", "headers": { "Accept": "*/*" } } }),
    );
    store.apply(
        "Network.responseReceivedExtraInfo",
        &json!({ "requestId": "x", "statusCode": 200, "headers": { "Set-Cookie": "a=1\nb=2" } }),
    );
    store.apply(
        "Network.responseReceived",
        &json!({ "requestId": "x", "response": { "status": 200, "headers": { "X": "y" } } }),
    );
    let e = store.get(1).unwrap();
    assert_eq!(
        crate::store::header(&e.req_headers, "cookie"),
        Some("sid=1")
    );
    assert_eq!(
        e.res_headers,
        vec![
            ("Set-Cookie".into(), "a=1".into()),
            ("Set-Cookie".into(), "b=2".into())
        ]
    );
}

#[test]
fn ring_buffer_drops_the_oldest() {
    let mut store = TrafficStore::new(3);
    for i in 0..5 {
        store.apply(
            "Network.requestWillBeSent",
            &json!({ "requestId": i.to_string(), "request": { "url": format!("https://a.test/{i}"), "method": "GET" } }),
        );
    }
    let ids: Vec<u64> = store.iter().map(|e| e.id).collect();
    assert_eq!(ids, [3, 4, 5]);
    assert_eq!(store.dropped, 2);
    assert!(store.get(2).is_none());
    assert_eq!(store.get(4).unwrap().url, "https://a.test/3");
    assert_eq!(store.id_for("0"), None);
}

#[tokio::test]
async fn search_finds_urls_headers_and_bodies() {
    let s = Setup::new(HashMap::from([(
        "2".to_string(),
        "{\"sku\":\"WIDGET-42\"}".to_string(),
    )]));
    s.ok("net.capture", json!({ "on": true })).await;
    s.request(
        "1",
        "GET",
        "https://a.test/",
        "Document",
        200,
        "text/html",
        1.0,
        1.0,
        1,
    );
    s.request(
        "2",
        "GET",
        "https://api.a.test/items?page=2",
        "XHR",
        200,
        "application/json",
        1.0,
        1.0,
        1,
    );
    settle().await;

    let out = s.ok("net.search", json!({ "query": "page=2" })).await;
    assert_eq!(out["total"], 1);
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .contains("  url: https://api.a.test/items?page=2")
    );

    let out = s
        .ok(
            "net.search",
            json!({ "query": "SECRET-token", "in": "headers" }),
        )
        .await;
    assert_eq!(out["total"], 2);
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .contains("req header Authorization: Bearer secret-token")
    );

    // Body search downloads the text bodies it does not have yet.
    let out = s
        .ok("net.search", json!({ "query": "widget-42", "in": "body" }))
        .await;
    assert_eq!(out["total"], 1, "{}", out["text"]);
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .contains("res body: {\"sku\":\"WIDGET-42\"}")
    );

    let out = s.ok("net.search", json!({ "query": "nothing-here" })).await;
    assert_eq!(out["text"], "no request contains \"nothing-here\"");
}

#[tokio::test]
async fn websocket_frames_are_recorded_and_paginated() {
    let s = Setup::new(HashMap::new());
    s.ok("net.capture", json!({ "on": true })).await;
    s.emit(
        "Network.webSocketCreated",
        json!({ "requestId": "w", "url": "ws://a.test/ws", "initiator": { "type": "script" } }),
    );
    s.emit(
        "Network.webSocketWillSendHandshakeRequest",
        json!({ "requestId": "w", "timestamp": 5.0, "wallTime": 1.0, "request": { "headers": { "Upgrade": "websocket" } } }),
    );
    s.emit(
        "Network.webSocketHandshakeResponseReceived",
        json!({ "requestId": "w", "timestamp": 5.01, "response": { "status": 101, "statusText": "Switching Protocols", "headers": {} } }),
    );
    for (i, (sent, text)) in [(true, "hello"), (false, "echo: hello"), (false, "tick\n1")]
        .iter()
        .enumerate()
    {
        let method = if *sent {
            "Network.webSocketFrameSent"
        } else {
            "Network.webSocketFrameReceived"
        };
        s.emit(
            method,
            json!({ "requestId": "w", "timestamp": 5.5 + i as f64, "response": { "opcode": 1, "mask": *sent, "payloadData": text } }),
        );
    }
    settle().await;

    let list = s.ok("net.list", json!({})).await;
    assert_eq!(
        list["text"],
        "tab 1: 1 captured, 1 match\n#1 GET 101 websocket 22B open ws://a.test/ws (3 frames)"
    );
    let out = s.ok("net.ws_frames", json!({ "id": 1, "limit": 2 })).await;
    assert_eq!(
        out["text"],
        "#1 websocket ws://a.test/ws — 3 frames\n\
         0 → text 5B +0.500s hello\n\
         1 ← text 11B +1.500s echo: hello\n\
         … 1 more: net_ws_frames cursor=2"
    );
    let out = s.ok("net.ws_frames", json!({ "id": 1, "cursor": 2 })).await;
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .ends_with("2 ← text 6B +2.500s tick\\n1")
    );
    assert_eq!(out["more"], false);

    s.emit(
        "Network.webSocketClosed",
        json!({ "requestId": "w", "timestamp": 9.0 }),
    );
    settle().await;
    let list = s.ok("net.list", json!({})).await;
    assert!(
        list["text"]
            .as_str()
            .unwrap()
            .contains("22B 4.0s ws://a.test/ws")
    );
}

#[tokio::test]
async fn har_export_inline_and_to_file() {
    let s = Setup::new(HashMap::new());
    s.ok("net.capture", json!({ "on": true })).await;
    s.emit(
        "Network.requestWillBeSent",
        json!({ "requestId": "p", "request": { "url": "https://api.a.test/items?x=1", "method": "POST",
                "headers": { "Content-Type": "application/json" }, "postData": "{\"a\":1}", "hasPostData": true },
                "timestamp": 1.0, "wallTime": 1_709_210_096.5, "type": "Fetch", "initiator": { "type": "script" } }),
    );
    s.emit(
        "Network.responseReceived",
        json!({ "requestId": "p", "type": "Fetch",
                "response": { "status": 201, "statusText": "Created", "mimeType": "application/json", "headers": { "Location": "/items/9" },
                              "protocol": "http/1.1", "remoteIPAddress": "127.0.0.1", "remotePort": 8080,
                              "timing": { "requestTime": 1.0, "dnsStart": -1, "dnsEnd": -1, "connectStart": 1.0, "connectEnd": 3.0,
                                          "sslStart": -1, "sslEnd": -1, "sendStart": 4.0, "sendEnd": 5.0, "receiveHeadersEnd": 25.0 } } }),
    );
    s.emit(
        "Network.loadingFinished",
        json!({ "requestId": "p", "timestamp": 1.030, "encodedDataLength": 300 }),
    );
    settle().await;

    let out = s.ok("net.har_export", json!({})).await;
    assert_eq!(out["entries"], 1);
    let har = &out["har"]["log"];
    assert_eq!(har["version"], "1.2");
    let e = &har["entries"][0];
    assert_eq!(e["startedDateTime"], "2024-02-29T12:34:56.500Z");
    assert_eq!(e["request"]["method"], "POST");
    assert_eq!(e["request"]["postData"]["text"], "{\"a\":1}");
    assert_eq!(
        e["request"]["queryString"],
        json!([{ "name": "x", "value": "1" }])
    );
    assert_eq!(e["response"]["status"], 201);
    assert_eq!(e["response"]["redirectURL"], "/items/9");
    assert_eq!(e["serverIPAddress"], "127.0.0.1");
    assert_eq!(e["timings"]["connect"], 2.0);
    assert_eq!(e["timings"]["wait"], 20.0);
    assert_eq!(e["timings"]["receive"].as_f64().unwrap().round(), 5.0);
    assert_eq!(e["time"].as_f64().unwrap().round(), 30.0);

    let dir = tempfile::tempdir().unwrap();
    s.net().set_har_dir(dir.path().to_path_buf());
    let out = s.ok("net.har_export", json!({ "path": "x/out.har" })).await;
    let path = out["path"].as_str().unwrap();
    assert!(path.ends_with("out.har"));
    let written: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(written["log"]["entries"][0]["response"]["status"], 201);
    assert!(out.get("har").is_none());
}

// ------------------------------------------------------------------ rules

#[tokio::test]
async fn rules_enable_fetch_and_answer_every_paused_request() {
    let s = Setup::new(HashMap::from([(
        "f5".to_string(),
        "{\"price\":10}".to_string(),
    )]));
    let block = s
        .ok(
            "net.rule_add",
            json!({ "match": { "url_glob": "*://ads.test/*" }, "action": "block" }),
        )
        .await;
    assert_eq!(block["id"], "r1");
    assert_eq!(
        block["text"],
        "r1 tab 1 request glob \"*://ads.test/*\" → block (0 hits)"
    );
    assert_eq!(
        s.calls("Fetch.enable").last().unwrap(),
        &json!({ "patterns": [{ "urlPattern": "*://ads.test/*", "requestStage": "Request" }] })
    );
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_regex": "/api/user$", "method": "GET" },
                "action": { "mock": { "status": 418, "body": "{\"name\":\"mock\"}" } } }),
    )
    .await;
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_glob": "*/api/*" }, "action": { "set_headers": { "X-Debug": "1", "accept": "application/json" } } }),
    )
    .await;
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_glob": "*/api/price", "stage": "response" },
                "action": { "rewrite_body": { "find": "10", "replace": "99" } } }),
    )
    .await;
    assert_eq!(
        s.calls("Fetch.enable").last().unwrap()["patterns"],
        json!([
            { "urlPattern": "*://ads.test/*", "requestStage": "Request" },
            { "urlPattern": "*", "requestStage": "Request" },
            { "urlPattern": "*/api/*", "requestStage": "Request" },
            { "urlPattern": "*/api/price", "requestStage": "Response" },
        ])
    );

    s.emit(
        "Fetch.requestPaused",
        paused_event("f1", "GET", "https://ads.test/banner.js"),
    );
    s.emit(
        "Fetch.requestPaused",
        paused_event("f2", "GET", "https://a.test/api/user"),
    );
    s.emit(
        "Fetch.requestPaused",
        paused_event("f3", "POST", "https://a.test/api/save"),
    );
    s.emit(
        "Fetch.requestPaused",
        paused_event("f4", "GET", "https://a.test/style.css"),
    );
    s.emit(
        "Fetch.requestPaused",
        response_paused("f5", "https://a.test/api/price", 200),
    );
    settle().await;

    assert_eq!(
        s.calls("Fetch.failRequest"),
        vec![json!({ "requestId": "f1", "errorReason": "BlockedByClient" })]
    );
    let fulfilled = s.calls("Fetch.fulfillRequest");
    assert_eq!(fulfilled.len(), 2);
    let mock = fulfilled.iter().find(|p| p["requestId"] == "f2").unwrap();
    assert_eq!(mock["responseCode"], 418);
    assert_eq!(
        B64.decode(mock["body"].as_str().unwrap()).unwrap(),
        b"{\"name\":\"mock\"}"
    );
    assert_eq!(
        mock["responseHeaders"],
        json!([
            { "name": "content-type", "value": "application/json" },
            { "name": "access-control-allow-origin", "value": "https://a.test" },
            { "name": "access-control-allow-credentials", "value": "true" },
        ])
    );
    let rewritten = fulfilled.iter().find(|p| p["requestId"] == "f5").unwrap();
    assert_eq!(
        B64.decode(rewritten["body"].as_str().unwrap()).unwrap(),
        b"{\"price\":99}"
    );
    assert_eq!(rewritten["responseCode"], 200);
    assert_eq!(
        rewritten["responseHeaders"],
        json!([{ "name": "Content-Type", "value": "application/json" }]),
        "content-length of the old body is dropped"
    );
    assert_eq!(
        s.calls("Fetch.getResponseBody"),
        vec![json!({ "requestId": "f5" })]
    );

    let continued = s.calls("Fetch.continueRequest");
    assert_eq!(continued.len(), 2, "{continued:?}");
    let with_headers = continued.iter().find(|p| p["requestId"] == "f3").unwrap();
    assert_eq!(
        with_headers["headers"],
        json!([
            { "name": "Origin", "value": "https://a.test" },
            { "name": "X-Debug", "value": "1" },
            { "name": "accept", "value": "application/json" },
        ])
    );
    assert!(
        continued.contains(&json!({ "requestId": "f4" })),
        "unmatched: plain continue"
    );

    let list = s.ok("net.rule_list", json!({})).await;
    assert_eq!(list["count"], 4);
    assert!(
        list["text"]
            .as_str()
            .unwrap()
            .contains("r2 tab 1 request GET regex \"/api/user$\" → mock 418 (1 hits)")
    );

    // Removing everything disables Fetch again.
    for id in ["r1", "r2", "r3", "r4"] {
        s.ok("net.rule_remove", json!({ "id": id })).await;
    }
    assert_eq!(s.calls("Fetch.disable").len(), 1);
    let err = s
        .call("net.rule_remove", json!({ "id": "r1" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn bad_rules_are_rejected() {
    let s = Setup::new(HashMap::new());
    let err = s
        .call("net.rule_add", json!({ "match": { "stage": "response" }, "action": { "redirect": { "url": "https://b.test/" } } }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = s
        .call(
            "net.rule_add",
            json!({ "match": { "url_regex": "(" }, "action": "block" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert!(s.calls("Fetch.enable").is_empty());
}

#[tokio::test]
async fn a_failing_answer_falls_back_to_continue() {
    let s = Setup::new(HashMap::new());
    // Response-stage header change, but continueResponse is "not supported" by this browser.
    s.fake.on_cdp(|_, method, _| match method {
        "Fetch.enable" | "Fetch.continueRequest" => Some(Ok(json!({}))),
        "Fetch.continueResponse" => Some(Err(bmcp_core::BackendError::Cdp("not supported".into()))),
        _ => None,
    });
    let mut events = s.ctx.events.subscribe();
    s.ok(
        "net.rule_add",
        json!({ "match": { "stage": "response" }, "action": { "remove_headers": ["content-length"] } }),
    )
    .await;
    s.emit(
        "Fetch.requestPaused",
        response_paused("f1", "https://a.test/x", 200),
    );
    settle().await;
    assert_eq!(s.calls("Fetch.continueResponse").len(), 1);
    assert_eq!(
        s.calls("Fetch.continueRequest"),
        vec![json!({ "requestId": "f1" })]
    );
    let warned =
        std::iter::from_fn(|| events.try_recv().ok()).any(|e| e.topic == "net.intercept_error");
    assert!(warned);
}

// ------------------------------------------------------------------ breakpoints

#[tokio::test]
async fn breakpoint_pauses_then_resume_with_edits() {
    let s = Setup::new(HashMap::new());
    let mut events = s.ctx.events.subscribe();
    let bp = s
        .ok(
            "net.breakpoint",
            json!({ "match": { "url_glob": "*/api/*", "method": "POST" } }),
        )
        .await;
    assert_eq!(bp["id"], "b1");

    let mut event = paused_event("f1", "POST", "https://a.test/api/save");
    event["request"]["postData"] = json!("{\"qty\":1}");
    s.emit("Fetch.requestPaused", event);
    s.emit(
        "Fetch.requestPaused",
        paused_event("f2", "GET", "https://a.test/api/list"),
    );
    settle().await;

    // GET does not match the POST breakpoint: it went through.
    assert_eq!(
        s.calls("Fetch.continueRequest"),
        vec![json!({ "requestId": "f2" })]
    );
    let paused = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.topic == "net.paused")
        .expect("net.paused event");
    assert_eq!(paused.data["pause_id"], "p2");
    assert_eq!(paused.data["rule"], "b1");
    assert_eq!(paused.data["method"], "POST");
    assert_eq!(paused.data["post_data"], "{\"qty\":1}");

    let list = s.ok("net.paused_list", json!({})).await;
    assert!(
        list["text"]
            .as_str()
            .unwrap()
            .starts_with("p2 b1 tab 1 request POST https://a.test/api/save ("),
        "{}",
        list["text"]
    );

    let out = s
        .ok(
            "net.resume",
            json!({ "pause_id": "p2", "edits": { "url": "https://a.test/api/save2",
                    "headers": { "X-Test": "1", "Origin": null }, "body": "{\"qty\":5}" } }),
        )
        .await;
    assert_eq!(out["done"], "continued");
    let sent = s.calls("Fetch.continueRequest").pop().unwrap();
    assert_eq!(
        sent,
        json!({ "requestId": "f1", "url": "https://a.test/api/save2",
                "headers": [{ "name": "Accept", "value": "*/*" }, { "name": "X-Test", "value": "1" }],
                "postData": B64.encode("{\"qty\":5}") })
    );
    let err = s
        .call("net.resume", json!({ "pause_id": "p2" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(s.ok("net.paused_list", json!({})).await["count"], 0);
}

#[tokio::test]
async fn response_breakpoint_can_replace_the_answer_or_drop() {
    let s = Setup::new(HashMap::from([(
        "f1".to_string(),
        "{\"ok\":true}".to_string(),
    )]));
    s.ok("net.breakpoint", json!({ "stage": "response" })).await;
    s.emit(
        "Fetch.requestPaused",
        response_paused("f1", "https://a.test/api/me", 200),
    );
    s.emit(
        "Fetch.requestPaused",
        response_paused("f2", "https://a.test/api/other", 200),
    );
    settle().await;
    let list = s.ok("net.paused_list", json!({})).await;
    assert_eq!(list["count"], 2);

    let err = s
        .call(
            "net.resume",
            json!({ "pause_id": "p2", "edits": { "url": "https://b.test/" } }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidParams,
        "url is already sent at the response stage"
    );

    // New status, original body kept.
    let out = s
        .ok(
            "net.resume",
            json!({ "pause_id": "p2", "edits": { "status": 500 } }),
        )
        .await;
    assert_eq!(out["done"], "fulfilled");
    let f = s.calls("Fetch.fulfillRequest").pop().unwrap();
    assert_eq!(f["responseCode"], 500);
    assert_eq!(
        B64.decode(f["body"].as_str().unwrap()).unwrap(),
        b"{\"ok\":true}"
    );

    let out = s
        .ok("net.resume", json!({ "pause_id": "p3", "action": "drop" }))
        .await;
    assert_eq!(out["done"], "dropped");
    assert_eq!(
        s.calls("Fetch.failRequest"),
        vec![json!({ "requestId": "f2", "errorReason": "Aborted" })]
    );
}

#[tokio::test(start_paused = true)]
async fn paused_requests_continue_by_themselves_after_60_seconds() {
    let s = Setup::new(HashMap::new());
    let mut events = s.ctx.events.subscribe();
    s.ok("net.breakpoint", json!({})).await;
    s.emit(
        "Fetch.requestPaused",
        paused_event("f1", "GET", "https://a.test/x"),
    );
    settle().await;
    assert_eq!(s.ok("net.paused_list", json!({})).await["count"], 1);

    tokio::time::advance(Duration::from_secs(59)).await;
    settle().await;
    assert!(s.calls("Fetch.continueRequest").is_empty(), "not yet");

    tokio::time::advance(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(
        s.calls("Fetch.continueRequest"),
        vec![json!({ "requestId": "f1" })]
    );
    let timeout = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.topic == "net.pause_timeout")
        .expect("warning event");
    assert_eq!(timeout.data["pause_id"], "p2");
    assert!(timeout.data["warning"].as_str().unwrap().contains("60 s"));
    assert_eq!(s.ok("net.paused_list", json!({})).await["count"], 0);
}

#[tokio::test]
async fn removing_a_breakpoint_releases_what_it_holds() {
    let s = Setup::new(HashMap::new());
    s.ok("net.breakpoint", json!({})).await;
    s.emit(
        "Fetch.requestPaused",
        paused_event("f1", "GET", "https://a.test/x"),
    );
    settle().await;
    let out = s.ok("net.rule_remove", json!({ "id": "b1" })).await;
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .ends_with("continued 1 paused requests")
    );
    assert_eq!(
        s.calls("Fetch.continueRequest"),
        vec![json!({ "requestId": "f1" })]
    );
    assert_eq!(s.calls("Fetch.disable").len(), 1);
}

// ------------------------------------------------------------------ shared CDP session

impl Setup {
    /// The tab's shared CDP state (the same instance the commands use).
    fn share(&self) -> Arc<CdpShare> {
        self.registry
            .extensions()
            .get_or_insert_with(CdpShare::default)
    }

    fn backend(&self) -> Arc<dyn BrowserBackend> {
        self.fake.clone()
    }

    /// CDP answers (`Fetch.*Request`, `continueResponse`) sent for one request id.
    fn answers_for(&self, id: &str) -> Vec<String> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, p)| {
                p["requestId"] == id
                    && matches!(
                        m.as_str(),
                        "Fetch.continueRequest"
                            | "Fetch.continueResponse"
                            | "Fetch.fulfillRequest"
                            | "Fetch.failRequest"
                    )
            })
            .map(|(m, _)| m)
            .collect()
    }
}

/// Review item: `net.capture off` sent `Network.disable` on the shared session and so ended
/// the traffic log of a running recording.
#[tokio::test]
async fn capture_off_keeps_network_on_while_a_recording_holds_it() {
    let s = Setup::new(HashMap::new());
    // What `record.start {traffic: true}` does.
    s.share()
        .acquire(&s.backend(), TAB, "Network", "record:rec-1", json!({}))
        .await
        .unwrap();
    s.ok("net.capture", json!({ "on": true })).await;
    // Net needs bigger post bodies: enable is sent again with the merged parameters.
    assert_eq!(
        s.calls("Network.enable"),
        vec![json!({}), json!({ "maxPostDataSize": 65536 })]
    );
    let out = s.ok("net.capture", json!({ "on": false })).await;
    assert_eq!(out["capturing"], false);
    assert!(s.calls("Network.disable").is_empty(), "record still logs");
    assert_eq!(s.share().holders(TAB, "Network"), ["record:rec-1"]);

    // The recording ends: now nobody needs Network.
    s.share()
        .release(&s.backend(), TAB, "Network", "record:rec-1")
        .await
        .unwrap();
    assert_eq!(s.calls("Network.disable").len(), 1);
}

/// Review item: a matching breakpoint used to win over earlier block/mock rules.
#[tokio::test]
async fn rules_and_breakpoints_apply_in_the_order_they_were_added() {
    let s = Setup::new(HashMap::new());
    let mut events = s.ctx.events.subscribe();
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_glob": "*://ads.test/*" }, "action": "block" }),
    )
    .await;
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_glob": "*/api/*" }, "action": { "set_headers": { "X-Debug": "1" } } }),
    )
    .await;
    s.ok("net.breakpoint", json!({})).await; // b3: everything
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_glob": "*/late/*" }, "action": "block" }),
    )
    .await;

    // r1 comes before b3: blocked, never paused.
    s.emit(
        "Fetch.requestPaused",
        paused_event("f1", "GET", "https://ads.test/banner.js"),
    );
    settle().await;
    assert_eq!(s.answers_for("f1"), ["Fetch.failRequest"]);
    assert_eq!(s.ok("net.paused_list", json!({})).await["count"], 0);

    // b3 holds it; the header rule before it is kept for the release.
    s.emit(
        "Fetch.requestPaused",
        paused_event("f2", "GET", "https://a.test/api/x"),
    );
    settle().await;
    let paused = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.topic == "net.paused")
        .expect("net.paused");
    assert_eq!(paused.data["rule"], "b3");
    assert_eq!(
        paused.data["pending_actions"],
        json!(["set_headers X-Debug"])
    );
    let pause_id = paused.data["pause_id"].as_str().unwrap().to_string();
    let out = s.ok("net.resume", json!({ "pause_id": pause_id })).await;
    assert_eq!(out["done"], "continued");
    let sent = s.calls("Fetch.continueRequest").pop().unwrap();
    assert_eq!(sent["requestId"], "f2");
    assert!(
        sent["headers"]
            .as_array()
            .unwrap()
            .contains(&json!({ "name": "X-Debug", "value": "1" })),
        "{sent}"
    );

    // b3 comes before r4: paused first, and releasing it without edits blocks it.
    s.emit(
        "Fetch.requestPaused",
        paused_event("f3", "GET", "https://a.test/late/x"),
    );
    settle().await;
    let paused = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.topic == "net.paused")
        .expect("net.paused");
    let pause_id = paused.data["pause_id"].as_str().unwrap().to_string();
    let out = s.ok("net.resume", json!({ "pause_id": pause_id })).await;
    assert_eq!(out["done"], "dropped");
    assert_eq!(s.answers_for("f3"), ["Fetch.failRequest"]);
}

/// Review item: `net.resume` returned early (no answer at all) when the original response
/// body could not be read, so the request hung.
#[tokio::test]
async fn resume_always_answers_even_when_the_body_cannot_be_read() {
    // No bodies: every Fetch.getResponseBody fails.
    let s = Setup::new(HashMap::new());
    s.ok("net.breakpoint", json!({ "stage": "response" })).await;
    let mut failed = paused_event("f2", "GET", "https://a.test/broken");
    failed["responseErrorReason"] = json!("ConnectionReset");
    s.emit("Fetch.requestPaused", failed);
    s.emit(
        "Fetch.requestPaused",
        response_paused("f3", "https://a.test/moved", 302),
    );
    s.emit(
        "Fetch.requestPaused",
        response_paused("f4", "https://a.test/api", 200),
    );
    settle().await;
    let list = s.ok("net.paused_list", json!({})).await;
    assert_eq!(list["count"], 3);
    let pause_of = |url: &str| -> String {
        let text = list["text"].as_str().unwrap();
        let line = text.lines().find(|l| l.contains(url)).unwrap();
        line.split(' ').next().unwrap().to_string()
    };

    // A failed response and a redirect have no body to keep: refused up front, still paused.
    for url in ["/broken", "/moved"] {
        let err = s
            .call(
                "net.resume",
                json!({ "pause_id": pause_of(url), "edits": { "status": 200 } }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams, "{url}: {err:?}");
    }
    assert_eq!(s.ok("net.paused_list", json!({})).await["count"], 3);
    // With a body it works.
    let out = s
        .ok(
            "net.resume",
            json!({ "pause_id": pause_of("/broken"), "edits": { "status": 200, "response_body": "ok" } }),
        )
        .await;
    assert_eq!(out["done"], "fulfilled");

    // The body read fails at resume time: an error, but the request is still released.
    let err = s
        .call(
            "net.resume",
            json!({ "pause_id": pause_of("/api"), "edits": { "status": 500 } }),
        )
        .await
        .unwrap_err();
    assert_ne!(err.code, ErrorCode::InvalidParams);
    assert_eq!(s.answers_for("f4"), ["Fetch.continueRequest"]);
    assert_eq!(s.ok("net.paused_list", json!({})).await["count"], 1);
}

/// Review item: when the capture listener fell behind, lost `Fetch.requestPaused` events
/// stranded their requests forever. Pauses now come from the Fetch coordinator, which sweeps
/// lost ones without disabling Fetch.
#[tokio::test(start_paused = true)]
async fn lost_pause_events_are_recovered_without_disabling_fetch() {
    let s = Setup::new(HashMap::new());
    let mut events = s.ctx.events.subscribe();
    s.ok("net.capture", json!({ "on": true })).await;
    s.ok(
        "net.rule_add",
        json!({ "match": { "url_glob": "*/api/*" }, "action": { "mock": { "body": "{}" } } }),
    )
    .await;
    s.emit(
        "Fetch.requestPaused",
        paused_event("interception-job-1.0", "GET", "https://a.test/api/a"),
    );
    settle().await;
    assert_eq!(
        s.answers_for("interception-job-1.0"),
        ["Fetch.fulfillRequest"]
    );

    // A burst the single-threaded test runtime cannot read in time: the oldest events
    // (the pause of job 2) are dropped from every subscriber's 1024-event channel.
    s.emit(
        "Fetch.requestPaused",
        paused_event("interception-job-2.0", "GET", "https://a.test/api/b"),
    );
    for i in 0..1100 {
        s.emit(
            "Network.dataReceived",
            json!({ "requestId": format!("n{i}"), "dataLength": 1 }),
        );
    }
    s.emit(
        "Fetch.requestPaused",
        paused_event("interception-job-3.0", "GET", "https://a.test/x"),
    );
    settle().await;
    assert!(s.answers_for("interception-job-2.0").is_empty());
    assert_eq!(
        s.answers_for("interception-job-3.0"),
        ["Fetch.continueRequest"]
    );
    let lagged = std::iter::from_fn(|| events.try_recv().ok()).any(|e| e.topic == "net.lagged");
    assert!(lagged, "the capture listener reports its own loss");

    tokio::time::sleep(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(
        s.answers_for("interception-job-2.0"),
        ["Fetch.continueRequest"],
        "swept"
    );
    assert_eq!(s.answers_for("interception-job-1.0").len(), 1);
    assert!(s.calls("Fetch.disable").is_empty());
    assert!(s.share().fetch_stats(TAB).swept >= 1);
}

// ------------------------------------------------------------------ replay

#[tokio::test]
async fn replay_refetches_from_the_page_with_edits() {
    let s = Setup::new(HashMap::new());
    s.ok("net.capture", json!({ "on": true })).await;
    s.emit(
        "Network.requestWillBeSent",
        json!({ "requestId": "1", "request": { "url": "https://a.test/api/items", "method": "POST",
                "headers": { "Content-Type": "application/json", "Cookie": "sid=1", ":path": "/api/items", "X-Token": "t" },
                "postData": "{\"q\":1}" }, "timestamp": 1.0, "wallTime": 1.0, "type": "Fetch", "initiator": { "type": "script" } }),
    );
    settle().await;
    s.fake.on_cdp(|_, method, _| match method {
        "Runtime.evaluate" => Some(Ok(json!({ "result": { "type": "object", "value": {
            "status": 201, "statusText": "Created", "type": "application/json", "length": 28, "body": "{\"id\":7,\"q\":2,\"name\":\"long\"}" } } }))),
        _ => None,
    });
    let out = s
        .ok("net.replay", json!({ "id": 1, "edits": { "body": "{\"q\":2}", "headers": { "X-Token": null, "X-New": "n" } }, "max_bytes": 16 }))
        .await;
    assert_eq!(out["status"], 201);
    assert_eq!(out["body"], "{\"id\":7,\"q\":2,\"n");
    assert_eq!(out["truncated"], true);
    let expr = s.calls("Runtime.evaluate").pop().unwrap();
    let code = expr["expression"].as_str().unwrap();
    assert!(
        code.contains("fetch(\"https://a.test/api/items\""),
        "{code}"
    );
    assert!(code.contains("method: \"POST\""));
    assert!(
        code.contains("headers: [[\"Content-Type\",\"application/json\"],[\"X-New\",\"n\"]]"),
        "{code}"
    );
    assert!(code.contains("body: \"{\\\"q\\\":2}\""));
    assert!(code.contains("credentials: 'include'"));
    assert_eq!(expr["awaitPromise"], true);
    assert_eq!(expr["returnByValue"], true);
}

// ------------------------------------------------------------------ api_to_code

#[tokio::test]
async fn api_to_code_clusters_endpoints_and_types_them() {
    let bodies = HashMap::from([
        (
            "1".to_string(),
            "{\"items\":[{\"id\":1,\"name\":\"a\"}],\"next\":2}".to_string(),
        ),
        ("2".to_string(), "{\"items\":[],\"next\":null}".to_string()),
        (
            "3".to_string(),
            "{\"id\":42,\"name\":\"x\",\"tags\":[\"t\"]}".to_string(),
        ),
        (
            "4".to_string(),
            "{\"id\":7,\"name\":\"y\",\"tags\":[]}".to_string(),
        ),
        ("5".to_string(), "{\"id\":8}".to_string()),
    ]);
    let s = Setup::new(bodies);
    s.ok("net.capture", json!({ "on": true })).await;
    s.request(
        "1",
        "GET",
        "http://127.0.0.1:8765/api/items?page=1&q=a",
        "XHR",
        200,
        "application/json",
        1.0,
        1.0,
        1,
    );
    s.request(
        "2",
        "GET",
        "http://127.0.0.1:8765/api/items?page=2",
        "Fetch",
        200,
        "application/json",
        1.0,
        1.0,
        1,
    );
    s.request(
        "3",
        "GET",
        "http://127.0.0.1:8765/api/items/42",
        "Fetch",
        200,
        "application/json",
        1.0,
        1.0,
        1,
    );
    s.request(
        "4",
        "GET",
        "http://127.0.0.1:8765/api/items/7",
        "Fetch",
        200,
        "application/json",
        1.0,
        1.0,
        1,
    );
    s.emit(
        "Network.requestWillBeSent",
        json!({ "requestId": "5", "request": { "url": "http://127.0.0.1:8765/api/items", "method": "POST",
                "headers": {}, "postData": "{\"name\":\"z\",\"price\":1.5}" }, "timestamp": 1.0, "wallTime": 1.0, "type": "Fetch" }),
    );
    s.respond(
        "5",
        "http://127.0.0.1:8765/api/items",
        "Fetch",
        201,
        "application/json",
        1.1,
        1,
    );
    // Not API traffic: ignored.
    s.request(
        "6",
        "GET",
        "http://127.0.0.1:8765/app.js",
        "Script",
        200,
        "text/javascript",
        1.0,
        1.0,
        1,
    );
    settle().await;

    let out = s
        .ok("net.api_to_code", json!({ "host": "127.0.0.1" }))
        .await;
    assert_eq!(out["endpoints"], 3);
    assert_eq!(out["requests"], 5);
    let code = out["code"].as_str().unwrap();
    assert!(
        code.contains("const BASE_URL = \"http://127.0.0.1:8765\";"),
        "{code}"
    );
    assert!(code.contains(
        "/** GET /api/items (seen 2×, status 200) */\n\
         export type GetApiItemsResponse = {\n  items: {\n    id: number;\n    name: string;\n  }[];\n  next: number | null;\n};\n\
         export function getApiItems(query: { page: number; q?: string }): Promise<GetApiItemsResponse> {\n  \
         return request(\"GET\", `/api/items`, { query });\n}"
    ), "{code}");
    assert!(code.contains(
        "export function getApiItemsById(id: string | number): Promise<GetApiItemsByIdResponse> {\n  \
         return request(\"GET\", `/api/items/${encodeURIComponent(String(id))}`);\n}"
    ), "{code}");
    assert!(
        code.contains("export type PostApiItemsBody = {\n  name: string;\n  price: number;\n};"),
        "{code}"
    );
    assert!(
        code.contains(
            "export function postApiItems(body: PostApiItemsBody): Promise<PostApiItemsResponse>"
        ),
        "{code}"
    );
    assert!(
        code.contains("return request(\"POST\", `/api/items`, { json: body });"),
        "{code}"
    );

    let err = s
        .call("net.api_to_code", json!({ "lang": "python" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = s
        .call("net.api_to_code", json!({ "host": "nowhere" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn every_command_is_in_the_net_toolset() {
    let mut registry = Registry::new();
    crate::register(&mut registry);
    let names: Vec<&str> = registry.list(None).iter().map(|c| c.name).collect();
    assert_eq!(names.len(), 14);
    assert!(
        registry
            .list(None)
            .iter()
            .all(|c| c.toolset == bmcp_core::Toolset::Net)
    );
    assert!(registry.list(Some(&[bmcp_core::Toolset::Core])).is_empty());
}
