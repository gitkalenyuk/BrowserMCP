//! OpenAI-compatible `chat/completions` with tools, streamed (SSE).
//!
//! The same code serves OpenAI, OpenRouter, Mistral, DeepSeek, xAI, Groq, Ollama, LM Studio and
//! any custom endpoint: `base_url` points at the API root that contains `/chat/completions`
//! (e.g. `https://api.openai.com/v1`, `http://localhost:11434/v1`).

use serde_json::{Value, json};

use super::sse::for_each_event;
use super::{
    Client, Message, ModelError, OnText, Reply, Request, ToolCall, check_status, parse_args,
    tool_schema,
};

/// Request body for one turn.
pub(super) fn body(req: Request<'_>) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": req.system })];
    for message in req.messages {
        messages.push(match message {
            Message::User { text } => json!({ "role": "user", "content": text }),
            Message::Assistant { text, calls, .. } => {
                let mut m = json!({ "role": "assistant", "content": text });
                // Some servers reject an empty `tool_calls` array, so only send it when needed.
                if !calls.is_empty() {
                    m["tool_calls"] = calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": { "name": c.name, "arguments": args_string(&c.args) },
                            })
                        })
                        .collect();
                }
                m
            }
            Message::Tool {
                call_id, output, ..
            } => json!({ "role": "tool", "tool_call_id": call_id, "content": output }),
        });
    }
    let mut body = json!({ "model": req.model, "messages": messages, "stream": true });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": tool_schema(&t.schema),
                    },
                })
            })
            .collect();
    }
    body
}

/// Arguments go back as the JSON string the model produced.
fn args_string(args: &Value) -> String {
    match args {
        Value::String(raw) => raw.clone(),
        other => other.to_string(),
    }
}

/// Accumulates streamed `delta`s into a reply.
#[derive(Default)]
pub(super) struct Accumulator {
    text: String,
    /// (id, name, arguments-so-far) per tool call index.
    calls: Vec<(String, String, String)>,
    stop: String,
}

impl Accumulator {
    /// Apply one `chat.completion.chunk` JSON object; returns new text, if any.
    pub(super) fn apply(&mut self, chunk: &Value) -> Result<Option<String>, ModelError> {
        if let Some(err) = chunk.get("error") {
            return Err(ModelError::Protocol(format!("stream error: {err}")));
        }
        let Some(choice) = chunk["choices"].get(0) else {
            return Ok(None); // e.g. a usage-only chunk
        };
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.stop = reason.to_string();
        }
        let delta = &choice["delta"];
        for (pos, call) in delta["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            // `index` is standard; a few servers omit it and send whole calls instead.
            let index = call["index"]
                .as_u64()
                .map(|i| i as usize)
                .unwrap_or(self.calls.len().max(pos));
            while self.calls.len() <= index {
                self.calls.push(Default::default());
            }
            let slot = &mut self.calls[index];
            if let Some(id) = call["id"].as_str() {
                slot.0 = id.to_string();
            }
            if let Some(name) = call["function"]["name"].as_str() {
                slot.1.push_str(name);
            }
            match &call["function"]["arguments"] {
                Value::String(part) => slot.2.push_str(part),
                // Non-standard: arguments as an object.
                Value::Object(_) => slot.2 = call["function"]["arguments"].to_string(),
                _ => {}
            }
        }
        match delta["content"].as_str() {
            Some(text) if !text.is_empty() => {
                self.text.push_str(text);
                Ok(Some(text.to_string()))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn finish(self) -> Reply {
        let calls = self
            .calls
            .into_iter()
            .enumerate()
            .filter(|(_, (_, name, _))| !name.is_empty())
            .map(|(i, (id, name, args))| ToolCall {
                id: if id.is_empty() {
                    format!("call_{i}")
                } else {
                    id
                },
                name,
                args: parse_args(&args),
            })
            .collect();
        Reply {
            text: self.text,
            calls,
            raw: None,
            stop: self.stop,
        }
    }
}

pub(super) async fn complete(
    client: &Client,
    req: Request<'_>,
    on_text: OnText<'_>,
) -> Result<Reply, ModelError> {
    let mut http = client
        .http
        .post(format!("{}/chat/completions", client.base_url))
        .json(&body(req));
    if let Some(key) = &client.api_key {
        http = http.bearer_auth(key);
    }
    let resp = check_status(http.send().await?).await?;

    // A server may ignore `stream: true` and answer with one JSON object.
    let is_json = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        let value: Value = resp.json().await?;
        return from_whole_response(&value, on_text);
    }

    let mut acc = Accumulator::default();
    for_each_event(resp, |event| {
        if event.data.trim() == "[DONE]" {
            return Ok(false);
        }
        let chunk: Value = serde_json::from_str(&event.data)
            .map_err(|e| ModelError::Protocol(format!("bad SSE chunk: {e}: {}", event.data)))?;
        if let Some(text) = acc.apply(&chunk)? {
            on_text(&text);
        }
        Ok(true)
    })
    .await?;
    Ok(acc.finish())
}

/// A non-streamed `chat.completion` object.
fn from_whole_response(value: &Value, on_text: OnText<'_>) -> Result<Reply, ModelError> {
    let message = &value["choices"][0]["message"];
    if message.is_null() {
        return Err(ModelError::Protocol(format!("no choices in {value}")));
    }
    // Same shape as a delta, so reuse the accumulator.
    let mut acc = Accumulator::default();
    let chunk = json!({ "choices": [{
        "delta": message,
        "finish_reason": value["choices"][0]["finish_reason"],
    }]});
    if let Some(text) = acc.apply(&chunk)? {
        on_text(&text);
    }
    Ok(acc.finish())
}

pub(super) async fn list_models(client: &Client) -> Result<Vec<String>, ModelError> {
    let mut http = client.http.get(format!("{}/models", client.base_url));
    if let Some(key) = &client.api_key {
        http = http.bearer_auth(key);
    }
    let value: Value = check_status(http.send().await?).await?.json().await?;
    Ok(value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}
