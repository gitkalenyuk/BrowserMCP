//! Deepgram live transcription: raw 16 kHz PCM goes over a WebSocket
//! (`wss://api.deepgram.com/v1/listen`), interim and final results come back while the user
//! speaks.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::CmdError;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::engines::query_string;

/// Text so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveText {
    /// Final segments in order.
    pub finals: Vec<String>,
    /// The segment still being recognized.
    pub interim: String,
    pub error: Option<String>,
    /// The server closed the connection.
    pub closed: bool,
}

impl LiveText {
    /// Everything heard so far, interim text included.
    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = self.finals.iter().map(String::as_str).collect();
        if !self.interim.trim().is_empty() {
            parts.push(self.interim.trim());
        }
        parts.join(" ")
    }

    /// Apply one message from Deepgram.
    pub fn apply(&mut self, msg: &Value) {
        if msg["type"] == "Results" {
            let transcript = msg["channel"]["alternatives"][0]["transcript"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            if msg["is_final"] == true {
                if !transcript.is_empty() {
                    self.finals.push(transcript);
                }
                self.interim.clear();
            } else {
                self.interim = transcript;
            }
        } else if msg["type"] == "Error" || msg.get("err_msg").is_some() {
            self.error = Some(
                msg["description"]
                    .as_str()
                    .or_else(|| msg["err_msg"].as_str())
                    .unwrap_or("Deepgram reported an error")
                    .to_string(),
            );
        }
    }
}

enum Outgoing {
    Audio(Vec<u8>),
    Finish,
}

/// One open live connection.
pub struct Live {
    tx: mpsc::UnboundedSender<Outgoing>,
    text: Arc<Mutex<LiveText>>,
    reader: tokio::task::JoinHandle<()>,
}

/// The `wss://…/v1/listen?…` URL for 16 kHz mono linear PCM.
pub fn live_url(base_url: &str, model: &str, language: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    let query = query_string(&[
        ("model", model.to_string()),
        ("encoding", "linear16".into()),
        ("sample_rate", "16000".into()),
        ("channels", "1".into()),
        ("interim_results", "true".into()),
        ("smart_format", "true".into()),
        ("punctuate", "true".into()),
        // nova-3 understands several languages in one stream with `multi`.
        (
            "language",
            language
                .filter(|l| *l != "auto")
                .unwrap_or("multi")
                .to_string(),
        ),
    ]);
    format!("{ws}/v1/listen?{query}")
}

fn tls_connector() -> Result<tokio_tungstenite::Connector, CmdError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| CmdError::internal(format!("TLS setup: {err}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
}

impl Live {
    /// Connect (fails fast when the key is wrong: Deepgram refuses the handshake).
    pub async fn connect(
        base_url: &str,
        api_key: &str,
        model: &str,
        language: Option<&str>,
    ) -> Result<Live, CmdError> {
        let url = live_url(base_url, model, language);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|err| CmdError::internal(format!("Deepgram URL: {err}")))?;
        request.headers_mut().insert(
            "Authorization",
            format!("Token {api_key}")
                .parse()
                .map_err(|_| CmdError::invalid_params("the Deepgram key has invalid characters"))?,
        );
        let connect = tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(tls_connector()?),
        );
        let (socket, _) = tokio::time::timeout(Duration::from_secs(10), connect)
            .await
            .map_err(|_| {
                CmdError::new(
                    bmcp_core::ErrorCode::Timeout,
                    "Deepgram did not answer in 10 s",
                )
            })?
            .map_err(|err| {
                let msg = match &err {
                    tokio_tungstenite::tungstenite::Error::Http(resp) => {
                        format!("Deepgram refused the connection: HTTP {}", resp.status())
                    }
                    other => format!("cannot connect to Deepgram: {other}"),
                };
                CmdError::internal(msg).with_hint("Check the Deepgram key in Settings → Voice.")
            })?;
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
        let text = Arc::new(Mutex::new(LiveText::default()));
        // Writer: audio as binary frames; CloseStream asks Deepgram to flush and close.
        tokio::spawn(async move {
            loop {
                let next = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await;
                let sent = match next {
                    // No audio for a while (paused UI): keep the connection alive.
                    Err(_) => sink.send(Message::text(r#"{"type":"KeepAlive"}"#)).await,
                    Ok(Some(Outgoing::Audio(bytes))) => sink.send(Message::binary(bytes)).await,
                    Ok(Some(Outgoing::Finish)) | Ok(None) => {
                        let _ = sink.send(Message::text(r#"{"type":"CloseStream"}"#)).await;
                        break;
                    }
                };
                if sent.is_err() {
                    break;
                }
            }
        });
        let reader = tokio::spawn({
            let text = text.clone();
            async move {
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(Message::Text(t)) => {
                            if let Ok(json) = serde_json::from_str::<Value>(t.as_str()) {
                                text.lock().unwrap_or_else(|e| e.into_inner()).apply(&json);
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(err) => {
                            text.lock().unwrap_or_else(|e| e.into_inner()).error =
                                Some(err.to_string());
                            break;
                        }
                    }
                }
                text.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
            }
        });
        Ok(Live { tx, text, reader })
    }

    /// Send 16-bit little-endian PCM.
    pub fn push(&self, pcm: Vec<u8>) {
        let _ = self.tx.send(Outgoing::Audio(pcm));
    }

    pub fn text(&self) -> LiveText {
        self.text.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Flush and wait (up to `timeout`) for the final results.
    pub async fn finish(self, timeout: Duration) -> LiveText {
        let _ = self.tx.send(Outgoing::Finish);
        let _ = tokio::time::timeout(timeout, self.reader).await;
        self.text.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Drop the connection without waiting.
    pub fn abort(self) {
        let _ = self.tx.send(Outgoing::Finish);
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_and_message_handling() {
        assert_eq!(
            live_url("https://api.deepgram.com/", "nova-3", Some("uk")),
            "wss://api.deepgram.com/v1/listen?model=nova-3&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true&punctuate=true&language=uk"
        );
        assert!(
            live_url("http://127.0.0.1:9", "nova-3", None)
                .starts_with("ws://127.0.0.1:9/v1/listen?")
        );
        assert!(live_url("x", "nova-3", Some("auto")).ends_with("language=multi"));
        let mut t = LiveText::default();
        let res = |text: &str, fin: bool| json!({"type":"Results","is_final":fin,"channel":{"alternatives":[{"transcript":text}]}});
        t.apply(&res("при", false));
        assert_eq!(t.text(), "при");
        t.apply(&res("привіт", true));
        t.apply(&res("як", false));
        assert_eq!(t.text(), "привіт як");
        t.apply(&res("", true));
        assert_eq!(t.text(), "привіт");
        t.apply(&json!({"type":"Metadata"}));
        t.apply(&json!({"type":"Error","description":"bad audio"}));
        assert_eq!(t.error.as_deref(), Some("bad audio"));
    }
}
