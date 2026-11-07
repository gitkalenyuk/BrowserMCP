//! One transcription request per engine: how it is built and how the answer is read.
//!
//! Building ([`build_request`]) and reading ([`parse_response`]) are pure functions, so the
//! tests check every engine's wire format without a network; [`transcribe`] sends the request
//! with reqwest.

use std::time::Duration;

use bmcp_core::{CmdError, ErrorCode};
use serde_json::Value;

use crate::multipart::Builder;
use crate::settings::EngineId;

/// Audio to transcribe.
#[derive(Debug, Clone, Copy)]
pub struct Audio<'a> {
    pub bytes: &'a [u8],
    /// `audio/wav`, `audio/webm`, …
    pub mime: &'a str,
}

/// Per-request options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Language code (`uk`, `en`); `None` = detect.
    pub language: Option<String>,
    /// Vocabulary / context hint (page title and site), where the engine takes one.
    pub prompt: Option<String>,
}

/// What an engine heard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub text: String,
    /// The language it detected or used (a code when known).
    pub language: Option<String>,
    /// Length of the audio as the engine reports it.
    pub duration_ms: Option<u64>,
}

/// A resolved engine: where to send audio and with which credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// OpenAI, Groq or a custom OpenAI-compatible `/audio/transcriptions`.
    OpenAiCompatible {
        engine: EngineId,
        base_url: String,
        api_key: Option<String>,
        model: String,
    },
    ElevenLabs {
        base_url: String,
        api_key: String,
        model: String,
    },
    Deepgram {
        base_url: String,
        api_key: String,
        model: String,
    },
    /// whisper.cpp's `whisper-server` on localhost.
    Whisper { base_url: String },
}

impl Endpoint {
    pub fn engine(&self) -> EngineId {
        match self {
            Endpoint::OpenAiCompatible { engine, .. } => *engine,
            Endpoint::ElevenLabs { .. } => EngineId::Elevenlabs,
            Endpoint::Deepgram { .. } => EngineId::Deepgram,
            Endpoint::Whisper { .. } => EngineId::Local,
        }
    }
}

/// A request ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// File name the APIs see (several of them pick the decoder by extension).
pub fn file_name(mime: &str) -> &'static str {
    let base = mime.split(';').next().unwrap_or("").trim();
    match base {
        "audio/webm" | "video/webm" => "audio.webm",
        "audio/ogg" | "audio/opus" => "audio.ogg",
        "audio/mpeg" | "audio/mp3" => "audio.mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "audio.m4a",
        "audio/flac" | "audio/x-flac" => "audio.flac",
        _ => "audio.wav",
    }
}

fn join(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Does this OpenAI-compatible model answer `verbose_json` (with language and duration)?
/// OpenAI's gpt-4o models answer only `json` / `text`.
fn verbose_json(model: &str) -> bool {
    !model.starts_with("gpt-")
}

pub fn build_request(endpoint: &Endpoint, audio: Audio<'_>, opts: &Options) -> HttpRequest {
    let language = opts.language.as_deref().filter(|l| *l != "auto");
    let prompt = opts.prompt.as_deref();
    match endpoint {
        Endpoint::OpenAiCompatible {
            base_url,
            api_key,
            model,
            ..
        } => {
            let form = Builder::new()
                .text("model", model)
                .text(
                    "response_format",
                    if verbose_json(model) {
                        "verbose_json"
                    } else {
                        "json"
                    },
                )
                .text("temperature", "0")
                .text_opt("language", language)
                .text_opt("prompt", prompt)
                .file("file", file_name(audio.mime), audio.mime, audio.bytes)
                .finish();
            let mut headers = vec![("Content-Type".to_string(), form.content_type)];
            if let Some(key) = api_key.as_deref().filter(|k| !k.is_empty()) {
                headers.push(("Authorization".into(), format!("Bearer {key}")));
            }
            HttpRequest {
                url: join(base_url, "audio/transcriptions"),
                headers,
                body: form.body,
            }
        }
        Endpoint::ElevenLabs {
            base_url,
            api_key,
            model,
        } => {
            // Scribe takes no free-text prompt.
            let form = Builder::new()
                .text("model_id", model)
                .text("tag_audio_events", "false")
                .text_opt("language_code", language)
                .file("file", file_name(audio.mime), audio.mime, audio.bytes)
                .finish();
            HttpRequest {
                url: join(base_url, "v1/speech-to-text"),
                headers: vec![
                    ("Content-Type".into(), form.content_type),
                    ("xi-api-key".into(), api_key.clone()),
                ],
                body: form.body,
            }
        }
        Endpoint::Deepgram {
            base_url,
            api_key,
            model,
        } => {
            let mut query = vec![
                ("model", model.clone()),
                ("smart_format", "true".into()),
                ("punctuate", "true".into()),
            ];
            match language {
                Some(lang) => query.push(("language", lang.to_string())),
                None => query.push(("detect_language", "true".into())),
            }
            HttpRequest {
                url: format!("{}?{}", join(base_url, "v1/listen"), query_string(&query)),
                headers: vec![
                    ("Content-Type".into(), audio.mime.to_string()),
                    ("Authorization".into(), format!("Token {api_key}")),
                ],
                body: audio.bytes.to_vec(),
            }
        }
        Endpoint::Whisper { base_url } => {
            let form = Builder::new()
                .text("response_format", "verbose_json")
                .text("temperature", "0.0")
                .text("temperature_inc", "0.2")
                .text("language", language.unwrap_or("auto"))
                .text_opt("prompt", prompt)
                .file("file", file_name(audio.mime), audio.mime, audio.bytes)
                .finish();
            HttpRequest {
                url: join(base_url, "inference"),
                headers: vec![("Content-Type".into(), form.content_type)],
                body: form.body,
            }
        }
    }
}

/// `a=1&b=x%20y`.
pub fn query_string(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Read an engine's answer (`status` = HTTP status).
pub fn parse_response(
    endpoint: &Endpoint,
    status: u16,
    body: &[u8],
) -> Result<Transcript, CmdError> {
    let engine = endpoint.engine();
    if !(200..300).contains(&status) {
        return Err(api_error(engine, status, body));
    }
    let json: Value = serde_json::from_slice(body).map_err(|err| {
        CmdError::internal(format!(
            "{} answered something that is not JSON: {err}",
            engine.name()
        ))
    })?;
    let secs = |v: &Value| v.as_f64().map(|s| (s * 1000.0).round() as u64);
    let transcript = match endpoint {
        Endpoint::OpenAiCompatible { .. } | Endpoint::Whisper { .. } => Transcript {
            text: json["text"].as_str().unwrap_or("").trim().to_string(),
            language: json["language"].as_str().map(language_code),
            duration_ms: secs(&json["duration"]),
        },
        Endpoint::ElevenLabs { .. } => Transcript {
            text: json["text"].as_str().unwrap_or("").trim().to_string(),
            language: json["language_code"].as_str().map(language_code),
            duration_ms: None,
        },
        Endpoint::Deepgram { .. } => {
            let channel = &json["results"]["channels"][0];
            Transcript {
                text: channel["alternatives"][0]["transcript"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                language: channel["detected_language"].as_str().map(language_code),
                duration_ms: secs(&json["metadata"]["duration"]),
            }
        }
    };
    Ok(Transcript {
        text: clean_text(&transcript.text),
        ..transcript
    })
}

/// Drop the markers Whisper prints for silence and noise (`[BLANK_AUDIO]`, `(music)`), and
/// squeeze whitespace.
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth_sq = 0usize;
    let mut kept = String::new();
    for c in text.chars() {
        match c {
            '[' => depth_sq += 1,
            ']' if depth_sq > 0 => depth_sq -= 1,
            _ if depth_sq == 0 => kept.push(c),
            _ => {}
        }
    }
    for word in kept.split_whitespace() {
        // Whole-word noise tags like "(music)" / "*laughs*".
        let tag = (word.starts_with('(') && word.ends_with(')'))
            || (word.len() > 2 && word.starts_with('*') && word.ends_with('*'));
        if tag {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Whisper / OpenAI report languages by English name (`ukrainian`): the common ones as codes.
pub fn language_code(lang: &str) -> String {
    let l = lang.trim().to_ascii_lowercase();
    const NAMES: &[(&str, &str)] = &[
        ("english", "en"),
        ("ukrainian", "uk"),
        ("russian", "ru"),
        ("german", "de"),
        ("french", "fr"),
        ("spanish", "es"),
        ("polish", "pl"),
        ("italian", "it"),
        ("portuguese", "pt"),
        ("dutch", "nl"),
        ("czech", "cs"),
        ("slovak", "sk"),
        ("romanian", "ro"),
        ("hungarian", "hu"),
        ("turkish", "tr"),
        ("japanese", "ja"),
        ("chinese", "zh"),
        ("korean", "ko"),
        ("arabic", "ar"),
        ("hindi", "hi"),
        ("belarusian", "be"),
        ("bulgarian", "bg"),
        ("swedish", "sv"),
        ("norwegian", "no"),
        ("danish", "da"),
        ("finnish", "fi"),
        ("greek", "el"),
        ("hebrew", "he"),
    ];
    NAMES
        .iter()
        .find(|(name, _)| *name == l)
        .map_or(l.clone(), |(_, code)| (*code).to_string())
}

/// A failed API call as a command error, with the provider's own message.
pub fn api_error(engine: EngineId, status: u16, body: &[u8]) -> CmdError {
    let json: Option<Value> = serde_json::from_slice(body).ok();
    let message = json
        .as_ref()
        .and_then(|j| {
            j["error"]["message"]
                .as_str()
                .or_else(|| j["error"].as_str())
                .or_else(|| j["detail"]["message"].as_str())
                .or_else(|| j["detail"].as_str())
                .or_else(|| j["err_msg"].as_str())
                .or_else(|| j["message"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body);
            text.chars()
                .take(300)
                .collect::<String>()
                .trim()
                .to_string()
        });
    let (code, hint) = match status {
        401 | 403 => (
            ErrorCode::InvalidParams,
            "Check the API key in Settings → Voice.",
        ),
        404 => (
            ErrorCode::NotFound,
            "Check the model name and the API address in Settings → Voice.",
        ),
        413 => (
            ErrorCode::InvalidParams,
            "The recording is too long for this engine.",
        ),
        429 => (
            ErrorCode::Timeout,
            "The engine is rate limiting or the account has no credit left; try again later.",
        ),
        _ => (
            ErrorCode::Internal,
            "Try again, or pick another engine in Settings → Voice.",
        ),
    };
    CmdError::new(
        code,
        format!("{} answered HTTP {status}: {message}", engine.name()),
    )
    .with_hint(hint)
}

/// The shared HTTP client (connect timeout; each request sets its own total timeout).
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("BrowserMCP/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// Send one transcription request.
pub async fn transcribe(
    client: &reqwest::Client,
    endpoint: &Endpoint,
    audio: Audio<'_>,
    opts: &Options,
) -> Result<Transcript, CmdError> {
    let req = build_request(endpoint, audio, opts);
    let mut builder = client
        .post(&req.url)
        .timeout(Duration::from_secs(180))
        .body(req.body);
    for (name, value) in &req.headers {
        builder = builder.header(name, value);
    }
    let engine = endpoint.engine();
    let response = builder.send().await.map_err(|err| {
        let local = matches!(endpoint, Endpoint::Whisper { .. });
        CmdError::new(
            if err.is_timeout() {
                ErrorCode::Timeout
            } else {
                ErrorCode::Internal
            },
            format!("cannot reach {}: {err}", engine.name()),
        )
        .with_hint(if local {
            "The local whisper-server did not answer; it is restarted on the next try."
        } else {
            "Check the internet connection (and the API address for a custom endpoint)."
        })
    })?;
    let status = response.status().as_u16();
    let body = response
        .bytes()
        .await
        .map_err(|err| CmdError::internal(format!("{}: {err}", engine.name())))?;
    parse_response(endpoint, status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multipart;

    fn fields(req: &HttpRequest) -> Vec<(String, Option<String>, Vec<u8>)> {
        multipart::parse(req.header("content-type").unwrap(), &req.body)
    }

    fn field<'a>(parts: &'a [(String, Option<String>, Vec<u8>)], name: &str) -> Option<&'a str> {
        parts
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, _, v)| std::str::from_utf8(v).unwrap())
    }

    const WAV: Audio<'static> = Audio {
        bytes: b"RIFF....WAVE",
        mime: "audio/wav",
    };

    #[test]
    fn openai_request_and_answer() {
        let ep = Endpoint::OpenAiCompatible {
            engine: EngineId::Openai,
            base_url: "https://api.openai.com/v1/".into(),
            api_key: Some("sk-1".into()),
            model: "gpt-4o-transcribe".into(),
        };
        let opts = Options {
            language: Some("uk".into()),
            prompt: Some("GitHub — github.com".into()),
        };
        let req = build_request(&ep, WAV, &opts);
        assert_eq!(req.url, "https://api.openai.com/v1/audio/transcriptions");
        assert_eq!(req.header("authorization"), Some("Bearer sk-1"));
        let parts = fields(&req);
        assert_eq!(field(&parts, "model"), Some("gpt-4o-transcribe"));
        assert_eq!(field(&parts, "response_format"), Some("json"));
        assert_eq!(field(&parts, "language"), Some("uk"));
        assert_eq!(field(&parts, "prompt"), Some("GitHub — github.com"));
        let file = parts.iter().find(|p| p.0 == "file").unwrap();
        assert_eq!(file.1.as_deref(), Some("audio.wav"));
        assert_eq!(file.2, WAV.bytes);
        let t = parse_response(&ep, 200, r#"{"text":" Привіт, світе. "}"#.as_bytes()).unwrap();
        assert_eq!(t.text, "Привіт, світе.");
        assert_eq!(t.language, None);
    }

    #[test]
    fn groq_and_whisper1_use_verbose_json_and_auto_omits_language() {
        let ep = Endpoint::OpenAiCompatible {
            engine: EngineId::Groq,
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key: Some("gsk".into()),
            model: "whisper-large-v3-turbo".into(),
        };
        let req = build_request(
            &ep,
            WAV,
            &Options {
                language: Some("auto".into()),
                prompt: None,
            },
        );
        let parts = fields(&req);
        assert_eq!(field(&parts, "response_format"), Some("verbose_json"));
        assert_eq!(field(&parts, "language"), None);
        assert_eq!(field(&parts, "prompt"), None);
        let t = parse_response(
            &ep,
            200,
            br#"{"text":"Hello there","language":"English","duration":2.5}"#,
        )
        .unwrap();
        assert_eq!(t.language.as_deref(), Some("en"));
        assert_eq!(t.duration_ms, Some(2500));
    }

    #[test]
    fn custom_without_key_sends_no_authorization() {
        let ep = Endpoint::OpenAiCompatible {
            engine: EngineId::Custom,
            base_url: "http://10.0.0.5:8000/v1".into(),
            api_key: None,
            model: "Systran/faster-whisper-small".into(),
        };
        let req = build_request(&ep, WAV, &Options::default());
        assert_eq!(req.url, "http://10.0.0.5:8000/v1/audio/transcriptions");
        assert_eq!(req.header("authorization"), None);
    }

    #[test]
    fn elevenlabs_request_and_answer() {
        let ep = Endpoint::ElevenLabs {
            base_url: "https://api.elevenlabs.io".into(),
            api_key: "xi".into(),
            model: "scribe_v1".into(),
        };
        let req = build_request(
            &ep,
            Audio {
                bytes: b"x",
                mime: "audio/webm;codecs=opus",
            },
            &Options {
                language: Some("uk".into()),
                prompt: Some("ignored".into()),
            },
        );
        assert_eq!(req.url, "https://api.elevenlabs.io/v1/speech-to-text");
        assert_eq!(req.header("xi-api-key"), Some("xi"));
        let parts = fields(&req);
        assert_eq!(field(&parts, "model_id"), Some("scribe_v1"));
        assert_eq!(field(&parts, "language_code"), Some("uk"));
        assert_eq!(field(&parts, "prompt"), None);
        assert_eq!(
            parts.iter().find(|p| p.0 == "file").unwrap().1.as_deref(),
            Some("audio.webm")
        );
        let t = parse_response(
            &ep,
            200,
            r#"{"language_code":"ukr","text":"Добрий день"}"#.as_bytes(),
        )
        .unwrap();
        assert_eq!(t.text, "Добрий день");
        assert_eq!(t.language.as_deref(), Some("ukr"));
    }

    #[test]
    fn deepgram_request_and_answer() {
        let ep = Endpoint::Deepgram {
            base_url: "https://api.deepgram.com".into(),
            api_key: "dg".into(),
            model: "nova-3".into(),
        };
        let req = build_request(&ep, WAV, &Options::default());
        assert_eq!(
            req.url,
            "https://api.deepgram.com/v1/listen?model=nova-3&smart_format=true&punctuate=true&detect_language=true"
        );
        assert_eq!(req.header("authorization"), Some("Token dg"));
        assert_eq!(req.header("content-type"), Some("audio/wav"));
        assert_eq!(req.body, WAV.bytes);
        let t = parse_response(
            &ep,
            200,
            r#"{"metadata":{"duration":1.25},"results":{"channels":[{"detected_language":"uk","alternatives":[{"transcript":"Тест"}]}]}}"#.as_bytes(),
        )
        .unwrap();
        assert_eq!(
            t,
            Transcript {
                text: "Тест".into(),
                language: Some("uk".into()),
                duration_ms: Some(1250)
            }
        );
    }

    #[test]
    fn whisper_server_request_and_noise_markers() {
        let ep = Endpoint::Whisper {
            base_url: "http://127.0.0.1:5000".into(),
        };
        let req = build_request(
            &ep,
            WAV,
            &Options {
                language: None,
                prompt: Some("Wikipedia".into()),
            },
        );
        assert_eq!(req.url, "http://127.0.0.1:5000/inference");
        let parts = fields(&req);
        assert_eq!(field(&parts, "language"), Some("auto"));
        assert_eq!(field(&parts, "prompt"), Some("Wikipedia"));
        let t = parse_response(
            &ep,
            200,
            br#"{"text":" [BLANK_AUDIO]\n Hello (music) world [ Silence ] ","language":"en","duration":3.0}"#,
        )
        .unwrap();
        assert_eq!(t.text, "Hello world");
        assert_eq!(t.language.as_deref(), Some("en"));
    }

    #[test]
    fn errors_carry_the_provider_message_and_a_hint() {
        let ep = Endpoint::OpenAiCompatible {
            engine: EngineId::Openai,
            base_url: "x".into(),
            api_key: None,
            model: "whisper-1".into(),
        };
        let err = parse_response(
            &ep,
            401,
            br#"{"error":{"message":"Incorrect API key provided"}}"#,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(
            err.message.contains("HTTP 401: Incorrect API key"),
            "{}",
            err.message
        );
        assert!(err.hint.as_deref().unwrap().contains("API key"));
        let err = api_error(
            EngineId::Elevenlabs,
            422,
            br#"{"detail":{"message":"bad file"}}"#,
        );
        assert!(err.message.contains("bad file"));
        let err = api_error(EngineId::Deepgram, 429, br#"{"err_msg":"slow down"}"#);
        assert_eq!(err.code, ErrorCode::Timeout);
        assert!(err.message.contains("slow down"));
        let err = parse_response(&ep, 200, b"<html>").unwrap_err();
        assert!(err.message.contains("not JSON"));
    }

    #[test]
    fn small_helpers() {
        assert_eq!(file_name("audio/ogg;codecs=opus"), "audio.ogg");
        assert_eq!(file_name("application/octet-stream"), "audio.wav");
        assert_eq!(language_code("Ukrainian"), "uk");
        assert_eq!(language_code("klingon"), "klingon");
        assert_eq!(
            query_string(&[("a", "x y".into()), ("b", "é".into())]),
            "a=x%20y&b=%C3%A9"
        );
    }
}
