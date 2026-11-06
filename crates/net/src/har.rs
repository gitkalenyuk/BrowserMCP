//! HAR 1.2 export (<http://www.softwareishard.com/blog/har-12-spec/>): the format DevTools,
//! Charles, Burp, Fiddler and k6 import.
//!
//! Bodies are included only when they are already in memory (`net.capture bodies:true`, or
//! fetched earlier by `net.get`), so an export never has to download hundreds of bodies.

use serde_json::{Value, json};

use crate::store::{Entry, Headers, header};

/// The whole HAR document for these entries (WebSockets are skipped: HAR has no frames).
pub fn build<'a>(entries: impl Iterator<Item = &'a Entry>) -> Value {
    let entries: Vec<Value> = entries
        .filter(|e| e.resource_type != "websocket")
        .map(entry)
        .collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "BrowserMCP", "version": env!("CARGO_PKG_VERSION") },
            "pages": [],
            "entries": entries,
        }
    })
}

fn entry(e: &Entry) -> Value {
    let timings = timings(e);
    let total: f64 = ["blocked", "dns", "connect", "send", "wait", "receive"]
        .iter()
        .filter_map(|k| timings[*k].as_f64())
        .filter(|v| *v > 0.0)
        .sum();
    let http_version = if e.protocol.is_empty() {
        "HTTP/1.1".to_string()
    } else {
        e.protocol.to_ascii_uppercase()
    };
    let mut request = json!({
        "method": e.method,
        "url": e.url,
        "httpVersion": http_version,
        "cookies": [],
        "headers": headers(&e.req_headers),
        "queryString": query_string(&e.url),
        "headersSize": -1,
        "bodySize": e.req_body.as_ref().map_or(0, |b| b.len() as i64),
    });
    if let Some(body) = &e.req_body {
        request["postData"] = json!({
            "mimeType": header(&e.req_headers, "content-type").unwrap_or(""),
            "text": body,
        });
    }
    let mut content = json!({
        "size": e.body.as_ref().map_or(0, |b| b.full_len),
        "mimeType": e.mime_type,
    });
    if let Some(body) = e.body.as_ref().filter(|b| !b.data.is_empty()) {
        content["text"] = json!(body.data);
        if body.base64 {
            content["encoding"] = json!("base64");
        }
        if body.truncated {
            content["comment"] = json!("truncated by BrowserMCP");
        }
    }
    let mut out = json!({
        "startedDateTime": iso8601(e.wall_time),
        "time": e.duration_ms().unwrap_or(total),
        "request": request,
        "response": {
            "status": e.status.unwrap_or(0),
            "statusText": e.status_text,
            "httpVersion": http_version,
            "cookies": [],
            "headers": headers(&e.res_headers),
            "content": content,
            "redirectURL": e.res_header("location").unwrap_or(""),
            "headersSize": -1,
            "bodySize": if e.size > 0 { e.size as i64 } else { -1 },
        },
        "cache": {},
        "timings": timings,
        "_resourceType": e.resource_type,
        "_initiator": e.initiator,
    });
    if let Some(ip) = &e.remote_ip {
        // HAR wants the address without the port.
        let ip = ip.rsplit_once(':').map_or(ip.as_str(), |(host, _)| host);
        out["serverIPAddress"] = json!(ip.trim_matches(['[', ']']));
    }
    if let Some(err) = &e.error {
        out["_error"] = json!(err);
    }
    out
}

/// HAR timings (ms) from CDP `ResourceTiming`. -1 = does not apply. `send`, `wait` and
/// `receive` must not be negative.
fn timings(e: &Entry) -> Value {
    let Some(t) = &e.timing else {
        let total = e.duration_ms().unwrap_or(0.0);
        return json!({ "blocked": -1, "dns": -1, "connect": -1, "ssl": -1,
                       "send": 0, "wait": total, "receive": 0 });
    };
    let at = |k: &str| t[k].as_f64().unwrap_or(-1.0);
    let span = |start: &str, end: &str| {
        let (s, e) = (at(start), at(end));
        if s >= 0.0 && e >= s { e - s } else { -1.0 }
    };
    let first_step = ["dnsStart", "connectStart", "sendStart"]
        .iter()
        .map(|k| at(k))
        .find(|v| *v >= 0.0)
        .unwrap_or(0.0);
    let send = span("sendStart", "sendEnd").max(0.0);
    let wait = (at("receiveHeadersEnd") - at("sendEnd")).max(0.0);
    // Total since `requestTime` (seconds, same clock as `end_ts`).
    let receive = match (e.end_ts, t["requestTime"].as_f64()) {
        (Some(end), Some(start)) => ((end - start) * 1000.0 - at("receiveHeadersEnd")).max(0.0),
        _ => 0.0,
    };
    json!({
        "blocked": first_step,
        "dns": span("dnsStart", "dnsEnd"),
        "connect": span("connectStart", "connectEnd"),
        "ssl": span("sslStart", "sslEnd"),
        "send": send,
        "wait": wait,
        "receive": receive,
    })
}

fn headers(list: &Headers) -> Value {
    Value::Array(
        list.iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}

/// `?a=1&b=x` → `[{name:"a",value:"1"},…]` (values as written, not decoded).
fn query_string(url: &str) -> Value {
    let query = url
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or_default())
        .unwrap_or_default();
    Value::Array(
        query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|pair| {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                json!({ "name": name, "value": value })
            })
            .collect(),
    )
}

/// Seconds since the Unix epoch → `2026-11-06T12:00:00.123Z`.
pub fn iso8601(epoch_seconds: f64) -> String {
    let millis = (epoch_seconds * 1000.0).round().max(0.0) as i64;
    let (days, ms_of_day) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let (y, m, d) = civil_from_days(days);
    let (h, min, s, ms) = (
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1000 % 60,
        ms_of_day % 1000,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{ms:03}Z")
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(iso8601(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_709_210_096.5), "2024-02-29T12:34:56.500Z");
    }

    #[test]
    fn query_strings() {
        assert_eq!(
            query_string("https://a.test/x?page=2&q=a%20b#frag"),
            json!([{"name":"page","value":"2"},{"name":"q","value":"a%20b"}])
        );
        assert_eq!(query_string("https://a.test/"), json!([]));
    }
}
