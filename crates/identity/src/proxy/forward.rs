//! Tunnels through an upstream proxy, and the local forwarder.
//!
//! Chromium cannot authenticate to SOCKS5 proxies (and asks the user for HTTP proxy
//! passwords in a dialog). The [`Forwarder`] fixes that: it listens on `127.0.0.1:<random>`
//! **without** authentication, speaks both SOCKS5 and HTTP proxy on that one port, and opens
//! every connection through the authenticated upstream ([`connect_via`]). Chromium is then
//! pointed at `socks5://127.0.0.1:<port>` (host names are passed on, so DNS is resolved by the
//! upstream and does not leak).
//!
//! Only loopback clients can reach it (it binds 127.0.0.1). It does not inspect traffic: after
//! the handshake bytes are copied both ways.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use super::config::{ProxyConfig, Scheme};

/// Upper limit for an HTTP request/response head we parse.
const MAX_HEAD: usize = 16 * 1024;
/// Handshake with the upstream must finish within this time.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

fn err(kind: io::ErrorKind, msg: impl Into<String>) -> io::Error {
    io::Error::new(kind, msg.into())
}

// ---------------------------------------------------------------- client side

/// Open a TCP stream to `host:port` through `upstream` (handshake included). The returned
/// stream carries the target's bytes.
pub async fn connect_via(upstream: &ProxyConfig, host: &str, port: u16) -> io::Result<TcpStream> {
    let work = async {
        let mut stream = TcpStream::connect((upstream.host.as_str(), upstream.port)).await?;
        stream.set_nodelay(true)?;
        match upstream.scheme {
            Scheme::Socks5 => socks5_handshake(&mut stream, upstream, host, port).await?,
            Scheme::Http => http_connect(&mut stream, upstream, host, port).await?,
        }
        Ok(stream)
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, work)
        .await
        .map_err(|_| err(io::ErrorKind::TimedOut, "proxy handshake timed out"))?
}

/// Connect directly or through a proxy.
pub async fn connect(via: Option<&ProxyConfig>, host: &str, port: u16) -> io::Result<TcpStream> {
    match via {
        Some(proxy) => connect_via(proxy, host, port).await,
        None => TcpStream::connect((host, port)).await,
    }
}

/// SOCKS5 address: IPv4, IPv6 or a domain name (resolved by the proxy).
fn socks_address(host: &str, port: u16) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            out.push(0x01);
            out.extend_from_slice(&ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            out.push(0x04);
            out.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let name = host.as_bytes();
            if name.is_empty() || name.len() > 255 {
                return Err(err(
                    io::ErrorKind::InvalidInput,
                    "host name too long for SOCKS5",
                ));
            }
            out.push(0x03);
            out.push(name.len() as u8);
            out.extend_from_slice(name);
        }
    }
    out.extend_from_slice(&port.to_be_bytes());
    Ok(out)
}

async fn socks5_handshake(
    s: &mut TcpStream,
    upstream: &ProxyConfig,
    host: &str,
    port: u16,
) -> io::Result<()> {
    // Greeting: offer "no auth" and, with credentials, "username/password".
    if upstream.has_auth() {
        s.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
    } else {
        s.write_all(&[0x05, 0x01, 0x00]).await?;
    }
    let mut reply = [0u8; 2];
    s.read_exact(&mut reply).await?;
    if reply[0] != 0x05 {
        return Err(err(
            io::ErrorKind::InvalidData,
            "upstream is not a SOCKS5 proxy",
        ));
    }
    match reply[1] {
        0x00 => {}
        0x02 => {
            let user = upstream.username.as_deref().unwrap_or("").as_bytes();
            let pass = upstream.password.as_deref().unwrap_or("").as_bytes();
            let mut auth = vec![0x01, user.len() as u8];
            auth.extend_from_slice(user);
            auth.push(pass.len() as u8);
            auth.extend_from_slice(pass);
            s.write_all(&auth).await?;
            let mut status = [0u8; 2];
            s.read_exact(&mut status).await?;
            if status[1] != 0x00 {
                return Err(err(
                    io::ErrorKind::PermissionDenied,
                    "SOCKS5 proxy rejected the user name or password",
                ));
            }
        }
        0xFF => {
            return Err(err(
                io::ErrorKind::PermissionDenied,
                if upstream.has_auth() {
                    "SOCKS5 proxy accepts none of our auth methods"
                } else {
                    "SOCKS5 proxy needs a user name and password"
                },
            ));
        }
        other => {
            return Err(err(
                io::ErrorKind::InvalidData,
                format!("SOCKS5 proxy chose unknown auth method {other}"),
            ));
        }
    }

    // CONNECT request.
    let mut req = vec![0x05, 0x01, 0x00];
    req.extend(socks_address(host, port)?);
    s.write_all(&req).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0x00 {
        return Err(err(
            io::ErrorKind::ConnectionRefused,
            format!(
                "SOCKS5 proxy could not connect: {}",
                socks_reply_text(head[1])
            ),
        ));
    }
    // Skip the bound address in the reply.
    let skip = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            len[0] as usize
        }
        _ => return Err(err(io::ErrorKind::InvalidData, "bad SOCKS5 reply address")),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest).await?;
    Ok(())
}

fn socks_reply_text(code: u8) -> &'static str {
    match code {
        0x01 => "general failure",
        0x02 => "not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown error",
    }
}

/// `user:pass` → Base64 for `Proxy-Authorization: Basic ...`.
pub fn basic_auth(user: &str, pass: &str) -> String {
    base64(format!("{user}:{pass}").as_bytes())
}

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

async fn http_connect(
    s: &mut TcpStream,
    upstream: &ProxyConfig,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let target = authority(host, port);
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(user) = &upstream.username {
        let pass = upstream.password.as_deref().unwrap_or("");
        req.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            basic_auth(user, pass)
        ));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await?;
    let head = read_head(s, Vec::new()).await?;
    let status = String::from_utf8_lossy(&head);
    let code = status.split_whitespace().nth(1).unwrap_or("");
    match code {
        "200" => Ok(()),
        "407" => Err(err(
            io::ErrorKind::PermissionDenied,
            "HTTP proxy rejected the credentials (407)",
        )),
        _ => Err(err(
            io::ErrorKind::ConnectionRefused,
            format!(
                "HTTP proxy refused CONNECT: {}",
                status.lines().next().unwrap_or("")
            ),
        )),
    }
}

/// Read until the end of an HTTP head (`\r\n\r\n`), one byte at a time so nothing of the body
/// or tunnel is consumed. `start` holds bytes already read.
async fn read_head<S: AsyncRead + Unpin>(s: &mut S, mut buf: Vec<u8>) -> io::Result<Vec<u8>> {
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if buf.len() > MAX_HEAD {
            return Err(err(io::ErrorKind::InvalidData, "HTTP head too large"));
        }
        if s.read(&mut byte).await? == 0 {
            return Err(err(
                io::ErrorKind::UnexpectedEof,
                "connection closed during HTTP head",
            ));
        }
        buf.push(byte[0]);
    }
    Ok(buf)
}

/// Health check: time to complete a tunnel to `host:port` through the proxy.
pub async fn check(
    upstream: &ProxyConfig,
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<Duration, String> {
    let started = Instant::now();
    match tokio::time::timeout(timeout, connect_via(upstream, host, port)).await {
        Ok(Ok(_stream)) => Ok(started.elapsed()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {} ms", timeout.as_millis())),
    }
}

// ---------------------------------------------------------------- the local forwarder

/// A running local forwarder. Dropping it stops it (and every open connection).
pub struct Forwarder {
    local: SocketAddr,
    upstream: ProxyConfig,
    task: tokio::task::JoinHandle<()>,
}

impl Forwarder {
    /// Listen on 127.0.0.1 with a random free port and forward to `upstream`.
    pub async fn start(upstream: ProxyConfig) -> io::Result<Forwarder> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local = listener.local_addr()?;
        let up = upstream.clone();
        let task = tokio::spawn(async move {
            // Owning the connections in a JoinSet means aborting this task closes them too.
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((client, _)) => {
                            let up = up.clone();
                            connections.spawn(async move {
                                if let Err(e) = serve_client(client, &up).await {
                                    tracing::debug!(proxy = %up, "forwarder connection: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("forwarder accept failed: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    },
                    // Reap finished connections so the set does not grow forever.
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Ok(Forwarder {
            local,
            upstream,
            task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    pub fn upstream(&self) -> &ProxyConfig {
        &self.upstream
    }

    /// `socks5://127.0.0.1:<port>`: what Chromium's proxy setting should be.
    pub fn socks5_url(&self) -> String {
        format!("socks5://{}", self.local)
    }

    /// `http://127.0.0.1:<port>` (same port, HTTP proxy protocol).
    pub fn http_url(&self) -> String {
        format!("http://{}", self.local)
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One client: SOCKS5 if the first byte is 5, else an HTTP proxy request.
async fn serve_client(mut client: TcpStream, upstream: &ProxyConfig) -> io::Result<()> {
    client.set_nodelay(true)?;
    let mut first = [0u8; 1];
    client.read_exact(&mut first).await?;
    let mut remote = if first[0] == 0x05 {
        socks5_server(&mut client, upstream).await?
    } else {
        http_server(&mut client, upstream, first[0]).await?
    };
    tokio::io::copy_bidirectional(&mut client, &mut remote).await?;
    Ok(())
}

/// Server side of SOCKS5 (no auth, CONNECT only). Returns the tunnel to the target.
async fn socks5_server(client: &mut TcpStream, upstream: &ProxyConfig) -> io::Result<TcpStream> {
    let mut n = [0u8; 1];
    client.read_exact(&mut n).await?;
    let mut methods = vec![0u8; n[0] as usize];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&0x00) {
        client.write_all(&[0x05, 0xFF]).await?;
        return Err(err(
            io::ErrorKind::PermissionDenied,
            "client wants auth; we offer none",
        ));
    }
    client.write_all(&[0x05, 0x00]).await?;

    let mut head = [0u8; 4];
    client.read_exact(&mut head).await?;
    let (host, port) = read_socks_target(client, head[3]).await?;
    if head[1] != 0x01 {
        reply_socks(client, 0x07).await?;
        return Err(err(
            io::ErrorKind::Unsupported,
            "only SOCKS5 CONNECT is supported",
        ));
    }
    match connect_via(upstream, &host, port).await {
        Ok(remote) => {
            reply_socks(client, 0x00).await?;
            Ok(remote)
        }
        Err(e) => {
            let code = match e.kind() {
                io::ErrorKind::ConnectionRefused => 0x05,
                io::ErrorKind::TimedOut => 0x04,
                _ => 0x01,
            };
            reply_socks(client, code).await?;
            Err(e)
        }
    }
}

async fn read_socks_target(client: &mut TcpStream, atyp: u8) -> io::Result<(String, u16)> {
    let host = match atyp {
        0x01 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        0x04 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| err(io::ErrorKind::InvalidData, "bad host name"))?
        }
        _ => {
            reply_socks(client, 0x08).await?;
            return Err(err(io::ErrorKind::InvalidData, "bad SOCKS5 address type"));
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    Ok((host, u16::from_be_bytes(port)))
}

async fn reply_socks<W: AsyncWrite + Unpin>(client: &mut W, code: u8) -> io::Result<()> {
    client
        .write_all(&[0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await
}

/// Server side of an HTTP proxy: `CONNECT host:port` or an absolute-form request
/// (`GET http://host/path`). Returns the connection to the target with any request bytes
/// already forwarded.
async fn http_server(
    client: &mut TcpStream,
    upstream: &ProxyConfig,
    first: u8,
) -> io::Result<TcpStream> {
    let head = read_head(client, vec![first]).await?;
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let (method, target, version) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or("HTTP/1.1"),
    );

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = parse_authority(target, 443)
            .ok_or_else(|| err(io::ErrorKind::InvalidData, "bad CONNECT target"))?;
        return match connect_via(upstream, &host, port).await {
            Ok(remote) => {
                client
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                Ok(remote)
            }
            Err(e) => {
                bad_gateway(client, &e).await?;
                Err(e)
            }
        };
    }

    // Absolute form: http://host[:port]/path → connect, send the request in origin form.
    let Some(rest) = target.strip_prefix("http://") else {
        client
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            )
            .await?;
        return Err(err(io::ErrorKind::InvalidData, "not a proxy request"));
    };
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = parse_authority(auth, 80)
        .ok_or_else(|| err(io::ErrorKind::InvalidData, "bad request URL"))?;
    let mut remote = match connect_via(upstream, &host, port).await {
        Ok(remote) => remote,
        Err(e) => {
            bad_gateway(client, &e).await?;
            return Err(e);
        }
    };
    // One request per connection: different requests may go to different hosts, so ask both
    // sides to close afterwards. Proxy-* headers are for us, not for the origin.
    let mut out = format!("{method} {path} {version}\r\n");
    for line in lines.filter(|l| !l.is_empty()) {
        let name = line
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if name.starts_with("proxy-") || name == "connection" || name == "keep-alive" {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    remote.write_all(out.as_bytes()).await?;
    Ok(remote)
}

async fn bad_gateway(client: &mut TcpStream, e: &io::Error) -> io::Result<()> {
    let body = format!("upstream proxy failed: {e}");
    client
        .write_all(
            format!(
                "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\nContent-Type: text/plain\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
}

/// `host:port` / `[v6]:port` / `host` → (host, port).
fn parse_authority(text: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default_port,
        };
        return Some((host.to_string(), port));
    }
    match text.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None if !text.is_empty() => Some((text.to_string(), default_port)),
        None => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            basic_auth("Aladdin", "open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn authorities() {
        assert_eq!(
            parse_authority("a.com:8443", 1),
            Some(("a.com".into(), 8443))
        );
        assert_eq!(parse_authority("a.com", 80), Some(("a.com".into(), 80)));
        assert_eq!(parse_authority("[::1]:5", 80), Some(("::1".into(), 5)));
        assert_eq!(parse_authority("a:b", 80), None);
    }

    // ------------------------------------------------ in-process fakes

    /// Origin server: answers every HTTP request with `hello from origin <path>` and echoes
    /// raw bytes after a `ECHO\n` line (to test tunnels).
    pub async fn origin() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 5];
                    if s.read_exact(&mut buf).await.is_err() {
                        return;
                    }
                    if &buf == b"ECHO\n" {
                        let mut data = [0u8; 1024];
                        while let Ok(n) = s.read(&mut data).await {
                            if n == 0 || s.write_all(&data[..n]).await.is_err() {
                                return;
                            }
                        }
                        return;
                    }
                    let Ok(head) = read_head(&mut s, buf).await else {
                        return;
                    };
                    let head = String::from_utf8_lossy(&head).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("?").to_string();
                    let saw_close = head.to_ascii_lowercase().contains("connection: close");
                    let saw_proxy_auth = head.to_ascii_lowercase().contains("proxy-authorization");
                    let body = format!(
                        "hello from origin {path} close={saw_close} proxy_auth={saw_proxy_auth}"
                    );
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(reply.as_bytes()).await;
                });
            }
        });
        addr
    }

    /// Fake authenticated SOCKS5 upstream (user "u", password "p"). Connects to the real
    /// target (only 127.0.0.1 in tests) and pipes.
    pub async fn fake_socks5(user: &'static str, pass: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let _ = async {
                        let mut g = [0u8; 2];
                        s.read_exact(&mut g).await?;
                        let mut m = vec![0u8; g[1] as usize];
                        s.read_exact(&mut m).await?;
                        if !m.contains(&0x02) {
                            s.write_all(&[0x05, 0xFF]).await?;
                            return Ok::<_, io::Error>(());
                        }
                        s.write_all(&[0x05, 0x02]).await?;
                        let mut v = [0u8; 2];
                        s.read_exact(&mut v).await?;
                        let mut u = vec![0u8; v[1] as usize];
                        s.read_exact(&mut u).await?;
                        let mut pl = [0u8; 1];
                        s.read_exact(&mut pl).await?;
                        let mut p = vec![0u8; pl[0] as usize];
                        s.read_exact(&mut p).await?;
                        if u != user.as_bytes() || p != pass.as_bytes() {
                            s.write_all(&[0x01, 0x01]).await?;
                            return Ok(());
                        }
                        s.write_all(&[0x01, 0x00]).await?;
                        let mut head = [0u8; 4];
                        s.read_exact(&mut head).await?;
                        let (host, port) = read_socks_target(&mut s, head[3]).await?;
                        let host = if host == "localhost" {
                            "127.0.0.1".to_string()
                        } else {
                            host
                        };
                        match TcpStream::connect((host.as_str(), port)).await {
                            Ok(mut remote) => {
                                reply_socks(&mut s, 0x00).await?;
                                tokio::io::copy_bidirectional(&mut s, &mut remote).await?;
                            }
                            Err(_) => reply_socks(&mut s, 0x05).await?,
                        }
                        Ok(())
                    }
                    .await;
                });
            }
        });
        addr
    }

    /// Fake HTTP CONNECT upstream requiring Basic auth.
    pub async fn fake_http_proxy(user: &'static str, pass: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let expected =
            format!("proxy-authorization: basic {}", basic_auth(user, pass)).to_ascii_lowercase();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let expected = expected.clone();
                tokio::spawn(async move {
                    let _ = async {
                        let head = read_head(&mut s, Vec::new()).await?;
                        let head = String::from_utf8_lossy(&head).to_string();
                        if !head.to_ascii_lowercase().contains(&expected) {
                            s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                                .await?;
                            return Ok::<_, io::Error>(());
                        }
                        let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                        let (host, port) = parse_authority(&target, 443).unwrap();
                        let mut remote = TcpStream::connect((host.as_str(), port)).await?;
                        s.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await?;
                        tokio::io::copy_bidirectional(&mut s, &mut remote).await?;
                        Ok(())
                    }
                    .await;
                });
            }
        });
        addr
    }

    fn proxy(url: &str) -> ProxyConfig {
        url.parse().unwrap()
    }

    async fn read_all(s: &mut TcpStream) -> String {
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).to_string()
    }

    #[tokio::test]
    async fn tunnels_through_authenticated_upstreams() {
        let origin = origin().await;
        let socks = fake_socks5("u", "p").await;
        let http = fake_http_proxy("u", "p").await;
        for up in [
            proxy(&format!("socks5://u:p@{socks}")),
            proxy(&format!("http://u:p@{http}")),
        ] {
            let mut s = connect_via(&up, "127.0.0.1", origin.port()).await.unwrap();
            s.write_all(b"ECHO\nping").await.unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping", "{up}");
        }
        // Wrong password is a clear error.
        for up in [
            proxy(&format!("socks5://u:wrong@{socks}")),
            proxy(&format!("http://u:wrong@{http}")),
        ] {
            let e = connect_via(&up, "127.0.0.1", origin.port())
                .await
                .unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{up}: {e}");
        }
        let e = connect_via(&proxy(&format!("socks5://{socks}")), "127.0.0.1", 1)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("needs a user name"), "{e}");
    }

    #[tokio::test]
    async fn forwarder_end_to_end() {
        let origin = origin().await;
        let socks = fake_socks5("u", "p").await;
        let http = fake_http_proxy("u", "p").await;
        for upstream in [
            proxy(&format!("socks5://u:p@{socks}")),
            proxy(&format!("http://u:p@{http}")),
        ] {
            let fwd = Forwarder::start(upstream.clone()).await.unwrap();
            assert!(fwd.local_addr().ip().is_loopback());
            let local: ProxyConfig = fwd.socks5_url().parse().unwrap();
            assert!(!local.has_auth());

            // 1. SOCKS5 without auth (what Chromium does), by domain name.
            let mut s = connect_via(&local, "localhost", origin.port())
                .await
                .unwrap();
            s.write_all(b"ECHO\nthrough socks").await.unwrap();
            let mut buf = [0u8; 13];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"through socks", "{upstream}");

            // 2. HTTP CONNECT without auth.
            let local_http: ProxyConfig = fwd.http_url().parse().unwrap();
            let mut s = connect_via(&local_http, "127.0.0.1", origin.port())
                .await
                .unwrap();
            s.write_all(b"ECHO\nconnect").await.unwrap();
            let mut buf = [0u8; 7];
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"connect");

            // 3. Plain HTTP absolute-form request.
            let mut s = TcpStream::connect(fwd.local_addr()).await.unwrap();
            let req = format!(
                "GET http://127.0.0.1:{}/page?q=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Connection: keep-alive\r\nProxy-Authorization: Basic eDp5\r\n\r\n",
                origin.port()
            );
            s.write_all(req.as_bytes()).await.unwrap();
            let reply = read_all(&mut s).await;
            assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
            assert!(
                reply.ends_with("hello from origin /page?q=1 close=true proxy_auth=false"),
                "{reply}"
            );

            // 4. A target that refuses: SOCKS reply code / 502.
            let dead = TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap()
                .local_addr()
                .unwrap();
            let e = connect_via(&local, "127.0.0.1", dead.port())
                .await
                .unwrap_err();
            assert!(e.to_string().contains("could not connect"), "{e}");
            let e = connect_via(&local_http, "127.0.0.1", dead.port())
                .await
                .unwrap_err();
            assert!(e.to_string().contains("502"), "{e}");

            // Stopping closes the port.
            let addr = fwd.local_addr();
            drop(fwd);
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                TcpStream::connect(addr).await.is_err(),
                "forwarder still listening"
            );
        }
    }

    #[tokio::test]
    async fn health_check() {
        let origin = origin().await;
        let socks = fake_socks5("u", "p").await;
        let good = proxy(&format!("socks5://u:p@{socks}"));
        assert!(
            check(&good, "127.0.0.1", origin.port(), Duration::from_secs(2))
                .await
                .is_ok()
        );
        let bad = proxy(&format!("socks5://u:x@{socks}"));
        let e = check(&bad, "127.0.0.1", origin.port(), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(e.contains("rejected"), "{e}");
        let closed = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let dead = proxy(&format!("socks5://{closed}"));
        assert!(
            check(&dead, "127.0.0.1", 80, Duration::from_secs(2))
                .await
                .is_err()
        );
    }
}
