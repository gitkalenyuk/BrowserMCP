//! IP → location lookup, used to fit an identity to a proxy's exit IP ("the timezone and
//! language should match the country the IP is in").
//!
//! Uses the free `ip-api.com` JSON endpoint. Its free tier is plain HTTP only (no key, 45
//! requests/minute), so the lookup itself needs no TLS and goes through any HTTP or SOCKS5
//! proxy as a simple tunnelled request. The answer is the proxy's *exit* IP, which is what
//! web sites see.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::config::ProxyConfig;
use super::forward;
use crate::identity::{accept_language, locale_for_country};

pub const DEFAULT_URL: &str = "http://ip-api.com/json/?fields=status,message,country,countryCode,regionName,city,lat,lon,timezone,isp,query";
const MAX_RESPONSE: usize = 64 * 1024;

/// Where an IP is, plus the locale and Accept-Language an identity there should use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeoInfo {
    /// The public IP the lookup service saw.
    pub ip: String,
    pub country: String,
    /// ISO code, e.g. "DE".
    pub country_code: String,
    pub region: String,
    pub city: String,
    /// IANA zone, e.g. "Europe/Berlin".
    pub timezone: String,
    pub latitude: f64,
    pub longitude: f64,
    pub isp: String,
    /// Suggested locale for the country ("de-DE"), if known.
    pub locale: Option<String>,
    pub accept_language: Option<String>,
}

/// The parts of ip-api's answer we read.
#[derive(Deserialize)]
struct IpApi {
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    country: String,
    #[serde(default, rename = "countryCode")]
    country_code: String,
    #[serde(default, rename = "regionName")]
    region: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    lat: f64,
    #[serde(default)]
    lon: f64,
    #[serde(default)]
    timezone: String,
    #[serde(default)]
    isp: String,
    #[serde(default)]
    query: String,
}

/// Look up the exit IP of `via` (or of this machine when `None`). `url` must be an `http://`
/// URL answering in ip-api's JSON format.
pub async fn lookup(
    via: Option<&ProxyConfig>,
    url: &str,
    timeout: Duration,
) -> Result<GeoInfo, String> {
    let work = async {
        let (host, port, path) = split_http_url(url)?;
        let mut stream = forward::connect(via, &host, port)
            .await
            .map_err(|e| format!("cannot reach {host}:{port}: {e}"))?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Mozilla/5.0\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        (&mut stream)
            .take(MAX_RESPONSE as u64)
            .read_to_end(&mut raw)
            .await
            .map_err(|e| format!("reading the answer: {e}"))?;
        parse_response(&raw)
    };
    tokio::time::timeout(timeout, work)
        .await
        .map_err(|_| format!("no answer within {} ms", timeout.as_millis()))?
}

/// `http://host[:port]/path` → (host, port, path).
fn split_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("the lookup URL must start with http:// (TLS is not supported here)")?;
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("bad port in {url}"))?),
        None => (auth, 80),
    };
    if host.is_empty() {
        return Err(format!("no host in {url}"));
    }
    Ok((host.to_string(), port, path.to_string()))
}

/// HTTP response bytes → GeoInfo (handles `Transfer-Encoding: chunked`).
fn parse_response(raw: &[u8]) -> Result<GeoInfo, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the answer is not HTTP")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let mut body = raw[split + 4..].to_vec();
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(format!("lookup service answered HTTP {status}"));
    }
    if head.contains("transfer-encoding: chunked") {
        body = dechunk(&body)?;
    }
    let api: IpApi =
        serde_json::from_slice(&body).map_err(|e| format!("unexpected answer: {e}"))?;
    if api.status != "success" {
        return Err(format!("lookup failed: {}", api.message));
    }
    let locale = locale_for_country(&api.country_code).map(str::to_string);
    Ok(GeoInfo {
        accept_language: locale.as_deref().map(accept_language),
        locale,
        ip: api.query,
        country: api.country,
        country_code: api.country_code,
        region: api.region,
        city: api.city,
        timezone: api.timezone,
        latitude: api.lat,
        longitude: api.lon,
        isp: api.isp,
    })
}

fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("bad chunked body")?;
        let size_text = String::from_utf8_lossy(&body[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "bad chunk size")?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&body[..size]);
        body = body.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::proxy::forward::tests::fake_socks5;
    use tokio::net::TcpListener;

    const ANSWER: &str = r#"{"status":"success","country":"Germany","countryCode":"DE","regionName":"Hesse","city":"Frankfurt am Main","lat":50.1109,"lon":8.68213,"timezone":"Europe/Berlin","isp":"Hetzner","query":"203.0.113.7"}"#;

    /// Fake ip-api: answers every request with [`ANSWER`] (chunked when asked).
    pub async fn fake_ip_api(chunked: bool) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let reply = if chunked {
                    let (a, b) = ANSWER.split_at(40);
                    format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
                        a.len(),
                        b.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{ANSWER}",
                        ANSWER.len()
                    )
                };
                let _ = s.write_all(reply.as_bytes()).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn looks_up_directly_and_through_a_proxy() {
        let api = fake_ip_api(false).await;
        let url = format!("http://127.0.0.1:{}/json/", api.port());
        let geo = lookup(None, &url, Duration::from_secs(2)).await.unwrap();
        assert_eq!(geo.ip, "203.0.113.7");
        assert_eq!(geo.timezone, "Europe/Berlin");
        assert_eq!(geo.locale.as_deref(), Some("de-DE"));
        assert_eq!(geo.accept_language.as_deref(), Some("de-DE,de,en-US,en"));

        let socks = fake_socks5("u", "p").await;
        let proxy: ProxyConfig = format!("socks5://u:p@{socks}").parse().unwrap();
        let chunked = fake_ip_api(true).await;
        let url = format!("http://127.0.0.1:{}/json/", chunked.port());
        let geo = lookup(Some(&proxy), &url, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(geo.city, "Frankfurt am Main");
    }

    #[test]
    fn reports_failures() {
        let fail = b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"fail\",\"message\":\"private range\"}";
        assert!(parse_response(fail).unwrap_err().contains("private range"));
        assert!(
            parse_response(b"HTTP/1.1 429 Too Many\r\n\r\n")
                .unwrap_err()
                .contains("429")
        );
        assert!(split_http_url("https://x/").is_err());
        assert_eq!(
            split_http_url("http://h:81/a?b").unwrap(),
            ("h".to_string(), 81, "/a?b".to_string())
        );
    }

    /// Real network: run with `cargo test -p bmcp-identity -- --ignored geo_live`.
    #[tokio::test]
    #[ignore = "needs internet access"]
    async fn geo_live() {
        let geo = lookup(None, DEFAULT_URL, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(!geo.ip.is_empty() && geo.timezone.contains('/'), "{geo:?}");
    }
}
