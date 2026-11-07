//! Model API adapters behind one small interface.
//!
//! The agent loop keeps the conversation as a list of provider-neutral [`Message`]s and asks a
//! [`Client`] for the next assistant turn. Each adapter converts that list to its own wire format:
//!
//! * [`Api::Anthropic`] — Messages API (`POST /v1/messages`, `x-api-key`), streamed over SSE;
//! * [`Api::OpenAi`] — `POST /chat/completions` with `tools`, streamed over SSE. Used for OpenAI,
//!   OpenRouter, Mistral, DeepSeek, xAI, Groq, Ollama, LM Studio and custom endpoints;
//! * [`Api::Gemini`] — native `generateContent` with function calling (not streamed).
//!
//! Provider-specific data that must be sent back unchanged (Anthropic thinking blocks with their
//! signatures, Gemini `thoughtSignature`s) is kept in [`Reply::raw`] / [`Message::Assistant::raw`].

mod anthropic;
mod gemini;
mod openai;
pub mod sse;

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Api {
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
    /// OpenAI-compatible `chat/completions` (OpenAI, OpenRouter, Ollama, LM Studio, vLLM…).
    #[serde(rename = "openai")]
    OpenAi,
    /// Google Gemini `generateContent`.
    Gemini,
}

/// A tool the model may call: one registry command.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDef {
    /// Wire name, e.g. `tabs_list`.
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (an object schema).
    pub schema: Value,
}

/// One tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider's call id; tool results refer to it.
    pub id: String,
    /// Wire name of the tool.
    pub name: String,
    /// Arguments. A JSON string (not object) means the model sent invalid JSON.
    pub args: Value,
}

/// One entry of the conversation, provider-neutral.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        text: String,
        calls: Vec<ToolCall>,
        /// The provider's own content (Anthropic blocks / Gemini parts), replayed as-is.
        raw: Option<Value>,
    },
    Tool {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
}

/// What the agent sends for one model turn.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolDef],
}

/// The model's answer for one turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub raw: Option<Value>,
    /// Provider's stop reason (`end_turn`, `tool_use`, `stop`, `STOP`…), for logs.
    pub stop: String,
}

impl Reply {
    /// The reply as a conversation entry.
    pub fn into_message(self) -> Message {
        Message::Assistant {
            text: self.text,
            calls: self.calls,
            raw: self.raw,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ModelError {
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Protocol(String),
}

impl From<reqwest::Error> for ModelError {
    fn from(err: reqwest::Error) -> Self {
        ModelError::Network(err.to_string())
    }
}

/// Callback for streamed text chunks.
pub type OnText<'a> = &'a mut (dyn FnMut(&str) + Send);

/// A configured connection to one provider.
#[derive(Debug, Clone)]
pub struct Client {
    pub api: Api,
    /// e.g. `https://api.openai.com/v1` (OpenAI-compatible: up to and including `/v1`),
    /// `https://api.anthropic.com`, `https://generativelanguage.googleapis.com`.
    pub base_url: String,
    pub api_key: Option<String>,
    http: reqwest::Client,
}

/// A header name and value as reqwest takes them, or why they are not valid HTTP.
pub fn header_pair(name: &str, value: &str) -> Result<(HeaderName, HeaderValue), String> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| "invalid header name")?;
    let value = HeaderValue::from_str(value).map_err(|_| "invalid header value")?;
    Ok((name, value))
}

fn http_client(headers: HeaderMap) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // Whole-request timeout; streamed agent turns can take minutes.
        .timeout(Duration::from_secs(600))
        .default_headers(headers)
        .build()
        .unwrap_or_default()
}

impl Client {
    pub fn new(api: Api, base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Client {
            api,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.filter(|k| !k.is_empty()),
            http: http_client(HeaderMap::new()),
        }
    }

    /// Send `headers` with every request (a provider's extra headers). The adapters' own
    /// headers (auth, `anthropic-version`, content type) win over these. Invalid pairs are
    /// skipped: `ai.provider_set` already rejects them.
    pub fn with_headers(mut self, headers: &BTreeMap<String, String>) -> Self {
        if headers.is_empty() {
            return self;
        }
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            match header_pair(name, value) {
                Ok((name, value)) => {
                    map.insert(name, value);
                }
                Err(why) => tracing::warn!(header = %name, why, "skipping an extra header"),
            }
        }
        self.http = http_client(map);
        self
    }

    /// Ask for the next assistant turn. Streamed text is passed to `on_text` as it arrives
    /// (Gemini: once, with the whole text).
    pub async fn complete(
        &self,
        req: Request<'_>,
        on_text: OnText<'_>,
    ) -> Result<Reply, ModelError> {
        match self.api {
            Api::Anthropic => anthropic::complete(self, req, on_text).await,
            Api::OpenAi => openai::complete(self, req, on_text).await,
            Api::Gemini => gemini::complete(self, req, on_text).await,
        }
    }

    /// Model ids the provider offers (free `GET …/models` call, used by `ai.provider_test`).
    pub async fn list_models(&self) -> Result<Vec<String>, ModelError> {
        match self.api {
            Api::Anthropic => anthropic::list_models(self).await,
            Api::OpenAi => openai::list_models(self).await,
            Api::Gemini => gemini::list_models(self).await,
        }
    }
}

/// Fail with the response body when the status is not 2xx.
async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, ModelError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(ModelError::Http {
        status: status.as_u16(),
        body: truncate(&body, 2000),
    })
}

/// The schema as tools expect it: without the `$schema` / `title` keys some APIs reject.
fn tool_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    schema
}

/// Tool-call arguments from a (possibly empty or broken) JSON string.
fn parse_args(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// At most `max` chars, with a marker when cut.
pub fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        None => text.to_string(),
        Some((cut, _)) => format!(
            "{}… [truncated, {} chars total]",
            &text[..cut],
            text.chars().count()
        ),
    }
}

#[cfg(test)]
mod tests;
