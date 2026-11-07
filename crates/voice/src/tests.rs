//! The commands against mock servers: no paid engine, GitHub or Hugging Face is ever called.
// tungstenite's handshake callbacks return its (large) HTTP response type as the error.
#![allow(clippy::result_large_err)]

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{Caller, CmdError, Ctx, ErrorCode, Registry};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::Digest;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::local::Sources;

struct Setup {
    registry: Registry,
    ctx: Ctx,
    service: Arc<VoiceService>,
    tmp: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        Self::with_sources(Sources::default())
    }

    fn with_sources(sources: Sources) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let mut registry = Registry::new();
        register(&mut registry);
        let service = registry
            .extensions()
            .insert(VoiceService::with_sources(tmp.path(), sources));
        let mut ctx = ctx(Caller::Ui, FakeBackend::new());
        ctx.extensions = registry.extensions().clone();
        Setup {
            registry,
            ctx,
            service,
            tmp,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry.call(self.ctx.clone(), name, args).await
    }

    fn stored(&self) -> Value {
        let text = std::fs::read_to_string(self.tmp.path().join("config").join("voice.json"))
            .unwrap_or_else(|_| "null".into());
        serde_json::from_str(&text).unwrap()
    }

    fn write_ai_json(&self, json: Value) {
        let dir = self.tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ai.json"), json.to_string()).unwrap();
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// One second of a 440 Hz tone, 16 kHz mono.
fn tone(ms: usize) -> Vec<i16> {
    (0..16 * ms)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8000.0) as i16)
        .collect()
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn form(req: &Request) -> Vec<(String, Option<String>, Vec<u8>)> {
    let ct = req
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    multipart::parse(&ct, &req.body)
}

fn field(req: &Request, name: &str) -> Option<String> {
    form(req)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, _, v)| String::from_utf8(v).unwrap())
}

#[tokio::test]
async fn settings_round_trip_without_leaking_keys() {
    let s = Setup::new();
    let out = s.call("voice.settings_get", json!({})).await.unwrap();
    assert_eq!(out["engine"], "local");
    assert_eq!(out["mode"], "hold");
    assert_eq!(out["hotkey"], "Space");
    assert_eq!(out["auto_submit_tap"], true);
    assert_eq!(out["openai"]["api_key_set"], false);
    let out = s
        .call(
            "voice.settings_set",
            json!({"engine": "openai", "mode": "tap", "hotkey": "alt+v", "language": "UK",
                   "openai": {"api_key": "sk-test-1234567890", "model": "whisper-1"},
                   "local": {"model": "base", "threads": 6, "idle_minutes": 3}}),
        )
        .await
        .unwrap();
    assert_eq!(out["engine"], "openai");
    assert_eq!(out["hotkey"], "Alt+V");
    assert_eq!(out["language"], "uk");
    assert_eq!(out["openai"]["api_key_set"], true);
    assert_eq!(out["openai"]["api_key_hint"], "…7890");
    assert!(!out.to_string().contains("sk-test"), "{out}");
    assert_eq!(out["local"]["threads"], 6);
    // Stored as plain JSON (like ai.json).
    let stored = s.stored();
    assert_eq!(stored["openai"]["api_key"], "sk-test-1234567890");
    assert_eq!(stored["local"]["model"], "base");
    // "" clears; bad values are refused and nothing changes.
    let out = s
        .call(
            "voice.settings_set",
            json!({"openai": {"api_key": ""}, "local": {"model": ""}}),
        )
        .await
        .unwrap();
    assert_eq!(out["openai"]["api_key_set"], false);
    assert_eq!(out["local"]["model"], Value::Null);
    for bad in [
        json!({"hotkey": "q"}),
        json!({"language": "klingon!"}),
        json!({"local": {"model": "huge"}}),
        json!({"local": {"idle_minutes": 0}}),
        json!({"custom": {"base_url": "ftp://x"}}),
        json!({"engine": "siri"}),
    ] {
        let err = s.call("voice.settings_set", bad.clone()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams, "{bad}");
    }
    assert_eq!(s.stored()["hotkey"], "Alt+V");
}

#[tokio::test]
async fn engines_report_what_is_ready_and_borrow_ai_keys() {
    let s = Setup::new();
    s.write_ai_json(json!({"providers": {"groq": {"api_key": "gsk_ai"}}}));
    s.call(
        "voice.settings_set",
        json!({"deepgram": {"api_key": "dg-key"}}),
    )
    .await
    .unwrap();
    let out = s.call("voice.engines", json!({})).await.unwrap();
    let by_id = |id: &str| {
        out["engines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(out["active"], "local");
    assert_eq!(by_id("local")["configured"], false);
    assert!(
        by_id("local")["reason"]
            .as_str()
            .unwrap()
            .contains("not downloaded")
    );
    assert_eq!(by_id("local")["offline"], true);
    assert_eq!(by_id("groq")["configured"], true);
    assert_eq!(by_id("groq")["key_source"], "ai_provider");
    assert_eq!(by_id("groq")["model"], "whisper-large-v3-turbo");
    assert_eq!(by_id("openai")["configured"], false);
    assert_eq!(by_id("deepgram")["configured"], true);
    assert_eq!(by_id("deepgram")["key_source"], "voice");
    assert_eq!(by_id("deepgram")["interim"], true);
    assert_eq!(by_id("custom")["configured"], false);
    let settings = s.call("voice.settings_get", json!({})).await.unwrap();
    assert_eq!(settings["groq"]["ai_provider_key"], true);
    assert_eq!(settings["openai"]["ai_provider_key"], false);
}

#[tokio::test]
async fn transcribe_with_openai_uses_the_ai_provider_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", "Bearer sk-from-ai"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text": " Відкрий пошту "})))
        .expect(1)
        .mount(&server)
        .await;
    let s = Setup::new();
    s.write_ai_json(json!({"providers": {"openai": {"api_key": "sk-from-ai",
        "base_url": format!("{}/v1", server.uri())}}}));
    s.call(
        "voice.settings_set",
        json!({"engine": "openai", "language": "uk"}),
    )
    .await
    .unwrap();
    let audio = wav::encode(&tone(1000), 16_000, 1);
    let out = s
        .call(
            "voice.transcribe",
            json!({"audio_base64": b64(&audio), "prompt": "Gmail — mail.google.com"}),
        )
        .await
        .unwrap();
    assert_eq!(out["text"], "Відкрий пошту");
    assert_eq!(out["engine"], "openai");
    assert_eq!(out["duration_ms"], 1000);
    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(field(req, "model").as_deref(), Some("gpt-4o-transcribe"));
    assert_eq!(field(req, "language").as_deref(), Some("uk"));
    assert_eq!(
        field(req, "prompt").as_deref(),
        Some("Gmail — mail.google.com")
    );
    let file = form(req).into_iter().find(|p| p.0 == "file").unwrap();
    assert_eq!(file.2, audio);
}

#[tokio::test]
async fn transcribe_errors_are_clear() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/speech-to-text"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"detail": {"message": "invalid api key"}})),
        )
        .mount(&server)
        .await;
    let s = Setup::new();
    let audio = b64(&wav::encode(&tone(500), 16_000, 1));
    // No key at all.
    let err = s
        .call(
            "voice.transcribe",
            json!({"audio_base64": audio, "engine": "elevenlabs"}),
        )
        .await
        .unwrap_err();
    assert!(err.message.contains("no API key"), "{}", err.message);
    s.call(
        "voice.settings_set",
        json!({"elevenlabs": {"api_key": "xi", "base_url": server.uri()}}),
    )
    .await
    .unwrap();
    let err = s
        .call(
            "voice.transcribe",
            json!({"audio_base64": audio, "engine": "elevenlabs"}),
        )
        .await
        .unwrap_err();
    assert!(
        err.message.contains("HTTP 401: invalid api key"),
        "{}",
        err.message
    );
    let err = s
        .call("voice.transcribe", json!({"audio_base64": "%%%"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    // Local Whisper is not downloaded.
    let err = s
        .call("voice.transcribe", json!({"audio_base64": audio}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.hint.unwrap().contains("Settings → Voice"));
}

#[tokio::test]
async fn deepgram_and_custom_batch_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .and(header("authorization", "Token dg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": {"duration": 0.5},
            "results": {"channels": [{"alternatives": [{"transcript": "hello world"}]}]}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"text": "custom ok", "language": "english"})),
        )
        .mount(&server)
        .await;
    let s = Setup::new();
    s.call(
        "voice.settings_set",
        json!({"deepgram": {"api_key": "dg", "base_url": server.uri()},
               "custom": {"base_url": format!("{}/v1/", server.uri()), "model": "my-whisper"}}),
    )
    .await
    .unwrap();
    let audio = b64(&wav::encode(&tone(500), 16_000, 1));
    let out = s
        .call(
            "voice.transcribe",
            json!({"audio_base64": audio, "engine": "deepgram", "language": "en"}),
        )
        .await
        .unwrap();
    assert_eq!(out["text"], "hello world");
    assert_eq!(out["duration_ms"], 500);
    let out = s
        .call(
            "voice.transcribe",
            json!({"audio_base64": audio, "engine": "custom"}),
        )
        .await
        .unwrap();
    assert_eq!(out["text"], "custom ok");
    assert_eq!(out["language"], "en");
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs[0].url.query().unwrap().contains("language=en"));
    assert_eq!(field(&reqs[1], "model").as_deref(), Some("my-whisper"));
    assert!(reqs[1].headers.get("authorization").is_none());
}

#[tokio::test]
async fn batch_stream_sends_the_audio_once_at_stop_and_cancel_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text": "три слова тут"})))
        .expect(1)
        .mount(&server)
        .await;
    let s = Setup::new();
    s.call(
        "voice.settings_set",
        json!({"engine": "groq", "groq": {"api_key": "gsk", "base_url": server.uri()}}),
    )
    .await
    .unwrap();
    let start = s
        .call("voice.stream_start", json!({"language": "uk"}))
        .await
        .unwrap();
    assert_eq!(start["interim"], false);
    let id = start["stream"].as_str().unwrap().to_string();
    let samples = tone(1000);
    for chunk in samples.chunks(4000) {
        let out = s
            .call(
                "voice.stream_push",
                json!({"stream": id, "pcm_base64": b64(&pcm_bytes(chunk))}),
            )
            .await
            .unwrap();
        assert_eq!(out["partial"], "");
    }
    let out = s
        .call("voice.stream_stop", json!({"stream": id}))
        .await
        .unwrap();
    assert_eq!(out["text"], "три слова тут");
    assert_eq!(out["duration_ms"], 1000);
    let req = &server.received_requests().await.unwrap()[0];
    let file = form(req).into_iter().find(|p| p.0 == "file").unwrap();
    assert_eq!(wav::parse(&file.2).unwrap().duration_ms(), 1000);
    // Cancel: discarded without an engine call (the mock expects exactly one request).
    let start = s.call("voice.stream_start", json!({})).await.unwrap();
    let id = start["stream"].as_str().unwrap().to_string();
    s.call(
        "voice.stream_push",
        json!({"stream": id, "pcm_base64": b64(&pcm_bytes(&samples))}),
    )
    .await
    .unwrap();
    let out = s
        .call("voice.stream_stop", json!({"stream": id, "cancel": true}))
        .await
        .unwrap();
    assert_eq!(out["text"], "");
    let err = s
        .call("voice.stream_push", json!({"stream": id, "pcm_base64": ""}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // Too short to be speech: no call either.
    let start = s.call("voice.stream_start", json!({})).await.unwrap();
    let id = start["stream"].as_str().unwrap().to_string();
    let out = s
        .call("voice.stream_stop", json!({"stream": id}))
        .await
        .unwrap();
    assert_eq!(out["text"], "");
    assert_eq!(s.service.open_streams(), 0);
}

#[tokio::test]
async fn stream_refuses_when_voice_is_off() {
    let s = Setup::new();
    s.call("voice.settings_set", json!({"enabled": false}))
        .await
        .unwrap();
    let err = s.call("voice.stream_start", json!({})).await.unwrap_err();
    assert!(err.message.contains("turned off"));
}

/// A fake Deepgram live endpoint: checks the key, echoes how many audio bytes arrived as
/// interim results, and answers CloseStream with a final result.
async fn fake_deepgram() -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut seen = Vec::new();
        let check = |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
            assert_eq!(req.headers()["authorization"], "Token dg-live");
            assert!(req.uri().query().unwrap().contains("encoding=linear16"));
            Ok(resp)
        };
        let mut ws = tokio_tungstenite::accept_hdr_async(tcp, check)
            .await
            .unwrap();
        let mut bytes = 0usize;
        while let Some(Ok(msg)) = ws.next().await {
            use tokio_tungstenite::tungstenite::Message;
            match msg {
                Message::Binary(b) => {
                    bytes += b.len();
                    let r = json!({"type":"Results","is_final":false,
                        "channel":{"alternatives":[{"transcript": format!("heard {bytes}")}]}});
                    ws.send(Message::text(r.to_string())).await.unwrap();
                }
                Message::Text(t) => {
                    seen.push(t.to_string());
                    if t.contains("CloseStream") {
                        let r = json!({"type":"Results","is_final":true,
                            "channel":{"alternatives":[{"transcript":"Привіт, Deepgram."}]}});
                        ws.send(Message::text(r.to_string())).await.unwrap();
                        ws.close(None).await.ok();
                        break;
                    }
                }
                _ => {}
            }
        }
        seen
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn deepgram_live_stream_shows_interim_text_and_the_final_result() {
    let (url, server) = fake_deepgram().await;
    let s = Setup::new();
    s.call(
        "voice.settings_set",
        json!({"engine": "deepgram", "deepgram": {"api_key": "dg-live", "base_url": url}}),
    )
    .await
    .unwrap();
    let start = s
        .call("voice.stream_start", json!({"language": "uk"}))
        .await
        .unwrap();
    assert_eq!(start["live"], true);
    assert_eq!(start["interim"], true);
    let id = start["stream"].as_str().unwrap().to_string();
    let chunk = b64(&pcm_bytes(&tone(250)));
    s.call(
        "voice.stream_push",
        json!({"stream": id, "pcm_base64": chunk}),
    )
    .await
    .unwrap();
    // The interim answer arrives asynchronously; it shows up in a later push.
    let mut partial = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let out = s
            .call("voice.stream_push", json!({"stream": id, "pcm_base64": ""}))
            .await
            .unwrap();
        partial = out["partial"].as_str().unwrap().to_string();
        if !partial.is_empty() {
            break;
        }
    }
    assert_eq!(partial, "heard 8000");
    let out = s
        .call("voice.stream_stop", json!({"stream": id}))
        .await
        .unwrap();
    assert_eq!(out["text"], "Привіт, Deepgram.");
    let seen = server.await.unwrap();
    assert!(seen.iter().any(|t| t.contains("CloseStream")));
}

#[tokio::test]
async fn deepgram_live_with_a_wrong_key_fails_at_start() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let reject = |_: &tokio_tungstenite::tungstenite::handshake::server::Request, _| {
            let resp = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(401)
                .body(None)
                .unwrap();
            Err(resp)
        };
        let _ = tokio_tungstenite::accept_hdr_async(tcp, reject).await;
    });
    let s = Setup::new();
    s.call(
        "voice.settings_set",
        json!({"engine": "deepgram", "deepgram": {"api_key": "bad", "base_url": format!("http://{addr}")}}),
    )
    .await
    .unwrap();
    let err = s.call("voice.stream_start", json!({})).await.unwrap_err();
    assert!(err.message.contains("HTTP 401"), "{}", err.message);
}

fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in files {
            z.start_file(*name, opts).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

fn sha_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn engine_download_picks_the_newest_build_checks_it_and_installs_it() {
    let server = MockServer::start().await;
    let zip = zip_with(&[
        ("Release/whisper-server.exe", b"MZ fake server"),
        ("Release/whisper-cli.exe", b"MZ fake cli"),
        ("Release/ggml.dll", b"dll"),
        ("../evil.txt", b"never written outside"),
    ]);
    let releases = json!([
        {"tag_name": "v1.9.5", "draft": false, "published_at": "2026-10-06T17:39:17Z", "assets": []},
        {"tag_name": "b5454", "draft": false, "published_at": "2026-10-06T16:51:27Z", "assets": [
            {"name": "whisper-bin-Win32.zip", "size": 1, "browser_download_url": format!("{}/dl/w32.zip", server.uri())},
            {"name": "whisper-bin-x64.zip", "size": zip.len(), "digest": format!("sha256:{}", sha_hex(&zip)),
             "browser_download_url": format!("{}/dl/x64.zip", server.uri())}]},
        {"tag_name": "b5130", "draft": false, "published_at": "2026-09-11T05:29:10Z", "assets": [
            {"name": "whisper-bin-x64.zip", "size": 5, "browser_download_url": format!("{}/dl/old.zip", server.uri())}]},
    ]);
    Mock::given(method("GET"))
        .and(path("/repos/ggml-org/whisper.cpp/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(releases))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/dl/x64.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(zip.clone()))
        .mount(&server)
        .await;
    let s = Setup::with_sources(Sources {
        github_api: server.uri(),
        models_base: server.uri(),
    });
    let out = s
        .call("voice.local_download", json!({"what": "engine"}))
        .await
        .unwrap();
    assert_eq!(out["download"]["what"], "engine");
    let done = s
        .service
        .local
        .wait_download(Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(done.phase, local::DownloadPhase::Done, "{done:?}");
    let status = s.call("voice.local_status", json!({})).await.unwrap();
    assert_eq!(status["engine"]["installed"], true);
    assert_eq!(status["engine"]["version"], "b5454");
    let bin = s.tmp.path().join("voice").join("bin");
    assert!(bin.join("whisper-cli.exe").is_file() && bin.join("ggml.dll").is_file());
    assert!(!s.tmp.path().join("voice").join("evil.txt").exists());
    assert!(!s.tmp.path().join("evil.txt").exists());
    assert!(!s.tmp.path().join("voice").join("tmp").exists());
    // Delete.
    let status = s
        .call("voice.local_delete", json!({"what": "engine"}))
        .await
        .unwrap();
    assert_eq!(status["engine"]["installed"], false);
}

#[tokio::test]
async fn a_download_that_does_not_match_the_published_hash_is_refused() {
    let server = MockServer::start().await;
    let zip = zip_with(&[("whisper-server.exe", b"MZ")]);
    Mock::given(method("GET"))
        .and(path("/repos/ggml-org/whisper.cpp/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"tag_name": "b1", "published_at": "2026-01-01T00:00:00Z", "assets": [
                {"name": "whisper-bin-x64.zip", "size": zip.len(), "digest": format!("sha256:{}", "0".repeat(64)),
                 "browser_download_url": format!("{}/dl/x64.zip", server.uri())}]}])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/dl/x64.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(zip))
        .mount(&server)
        .await;
    // A model whose file is not the published size.
    Mock::given(method("GET"))
        .and(path("/ggml-tiny.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"too small".to_vec()))
        .mount(&server)
        .await;
    let s = Setup::with_sources(Sources {
        github_api: server.uri(),
        models_base: server.uri(),
    });
    s.call("voice.local_download", json!({"what": "engine"}))
        .await
        .unwrap();
    let done = s
        .service
        .local
        .wait_download(Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(done.phase, local::DownloadPhase::Failed);
    assert!(done.error.unwrap().contains("SHA-256 mismatch"));
    assert!(!s.service.local.engine_installed());
    s.call("voice.local_download", json!({"what": "tiny"}))
        .await
        .unwrap();
    let done = s
        .service
        .local
        .wait_download(Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(done.phase, local::DownloadPhase::Failed);
    assert!(done.error.unwrap().contains("size mismatch"));
    let models = s.tmp.path().join("voice").join("models");
    assert_eq!(
        std::fs::read_dir(&models).unwrap().count(),
        0,
        "the .part file is removed"
    );
    let err = s
        .call("voice.local_download", json!({"what": "huge"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn a_download_can_be_cancelled() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ggml-base.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0u8; 1024])
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;
    let s = Setup::with_sources(Sources {
        github_api: server.uri(),
        models_base: server.uri(),
    });
    s.call("voice.local_download", json!({"what": "base"}))
        .await
        .unwrap();
    // A second download while one runs is refused.
    let err = s
        .call("voice.local_download", json!({"what": "tiny"}))
        .await
        .unwrap_err();
    assert!(err.message.contains("still downloading"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    s.call("voice.local_cancel", json!({})).await.unwrap();
    let done = s
        .service
        .local
        .wait_download(Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(done.phase, local::DownloadPhase::Cancelled);
}

#[tokio::test]
async fn local_status_lists_the_catalog_and_picks_the_best_installed_model() {
    let s = Setup::new();
    let status = s.call("voice.local_status", json!({})).await.unwrap();
    let models = status["models"].as_array().unwrap();
    assert_eq!(models.len(), catalog::MODELS.len());
    assert!(models.iter().all(|m| m["installed"] == false));
    assert_eq!(status["active_model"], Value::Null);
    // Fake an installed "tiny" (only the size is checked for "installed").
    let tiny = catalog::model("tiny").unwrap();
    let dir = s.tmp.path().join("voice").join("models");
    std::fs::create_dir_all(&dir).unwrap();
    let f = std::fs::File::create(dir.join(tiny.file)).unwrap();
    f.set_len(tiny.size).unwrap();
    let status = s.call("voice.local_status", json!({})).await.unwrap();
    assert_eq!(status["active_model"], "tiny");
    // Choosing a model that is not installed keeps using the installed one.
    s.call("voice.settings_set", json!({"local": {"model": "small"}}))
        .await
        .unwrap();
    let status = s.call("voice.local_status", json!({})).await.unwrap();
    assert_eq!(status["active_model"], "tiny");
    s.call("voice.local_delete", json!({"what": "tiny"}))
        .await
        .unwrap();
    assert!(!dir.join(tiny.file).exists());
}

/// Real download from GitHub and Hugging Face (the same code as the Download buttons): set
/// `BMCP_VOICE_TEST_DOWNLOAD` to a data folder, optionally `BMCP_VOICE_TEST_MODEL` (default
/// `base`). Skipped otherwise: the tests never download by themselves.
#[tokio::test]
async fn real_download_of_the_engine_and_a_model() {
    let Ok(data) = std::env::var("BMCP_VOICE_TEST_DOWNLOAD") else {
        eprintln!("skipped: BMCP_VOICE_TEST_DOWNLOAD not set");
        return;
    };
    let model = std::env::var("BMCP_VOICE_TEST_MODEL").unwrap_or_else(|_| "base".into());
    let service = Arc::new(VoiceService::new(std::path::Path::new(&data)));
    for what in ["engine", model.as_str()] {
        let installed = if what == "engine" {
            service.local.engine_installed()
        } else {
            service.local.model_installed(catalog::model(what).unwrap())
        };
        if installed {
            eprintln!("{what}: already installed");
            continue;
        }
        service.local.start_download(what).unwrap();
        let done = service
            .local
            .wait_download(Duration::from_secs(1800))
            .await
            .unwrap();
        eprintln!("{what}: {done:?}");
        assert_eq!(done.phase, local::DownloadPhase::Done, "{done:?}");
    }
    let status = service.local.status(&Default::default());
    eprintln!("{}", serde_json::to_string_pretty(&status).unwrap());
    assert!(status.engine.installed);
    assert_eq!(status.active_model.as_deref(), Some(model.as_str()));
}

/// Real-device check with the actual whisper.cpp build: set `BMCP_VOICE_TEST_DATA` to a data
/// folder whose `voice/` has the engine and a model, and `BMCP_VOICE_TEST_WAV` to a WAV file
/// (16 kHz mono). Skipped otherwise (CI has neither).
#[tokio::test]
async fn real_local_whisper_transcribes_a_wav_file() {
    let (Ok(data), Ok(wav_path)) = (
        std::env::var("BMCP_VOICE_TEST_DATA"),
        std::env::var("BMCP_VOICE_TEST_WAV"),
    ) else {
        eprintln!("skipped: BMCP_VOICE_TEST_DATA / BMCP_VOICE_TEST_WAV not set");
        return;
    };
    let mut registry = Registry::new();
    register(&mut registry);
    let service = registry
        .extensions()
        .insert(VoiceService::new(std::path::Path::new(&data)));
    let mut c = ctx(Caller::Ui, FakeBackend::new());
    c.extensions = registry.extensions().clone();
    let audio = std::fs::read(&wav_path).unwrap();
    let expected = std::env::var("BMCP_VOICE_TEST_EXPECT").unwrap_or_default();
    let out = registry
        .call(
            c.clone(),
            "voice.transcribe",
            json!({"audio_base64": b64(&audio), "language": "en", "engine": "local"}),
        )
        .await
        .unwrap();
    eprintln!("local whisper: {out}");
    let text = out["text"].as_str().unwrap().to_lowercase();
    for word in expected.split_whitespace() {
        assert!(
            text.contains(&word.to_lowercase()),
            "{word:?} not in {text:?}"
        );
    }
    // The same audio through a stream (interim passes + final).
    let start = registry
        .call(
            c.clone(),
            "voice.stream_start",
            json!({"language": "en", "engine": "local"}),
        )
        .await
        .unwrap();
    let id = start["stream"].as_str().unwrap().to_string();
    let info = wav::parse(&audio).unwrap();
    let pcm = &audio[audio.len() - info.data_len as usize..];
    let mut partials = Vec::new();
    for chunk in pcm.chunks(8000) {
        let out = registry
            .call(
                c.clone(),
                "voice.stream_push",
                json!({"stream": id, "pcm_base64": b64(chunk)}),
            )
            .await
            .unwrap();
        partials.push(out["partial"].as_str().unwrap().to_string());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let out = registry
        .call(c.clone(), "voice.stream_stop", json!({"stream": id}))
        .await
        .unwrap();
    eprintln!("stream partials: {partials:?}\nstream final: {out}");
    assert!(partials.iter().any(|p| !p.is_empty()), "no interim text");
    let status = service.local.status(&Default::default());
    assert!(
        status.server.is_some(),
        "the server keeps running until idle"
    );
    service.shutdown();
    assert!(service.local.server_info().is_none());
}
