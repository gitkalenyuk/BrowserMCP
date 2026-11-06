//! Anthropic Messages API (`POST {base}/v1/messages`), streamed.
//!
//! Streaming events: `message_start`, `content_block_start` / `_delta` / `_stop`,
//! `message_delta` (stop reason), `message_stop`, `ping`, `error`. Content blocks are rebuilt
//! exactly as the API sent them (including `thinking` blocks and their `signature`) and replayed
//! unchanged on the next request, which the API requires for thinking models.

use serde_json::{Value, json};

use super::sse::for_each_event;
use super::{
    Client, Message, ModelError, OnText, Reply, Request, ToolCall, check_status, parse_args,
    tool_schema,
};

const API_VERSION: &str = "2023-06-01";
/// Output limit per turn. Streaming keeps large values safe from HTTP timeouts.
const MAX_TOKENS: u32 = 32_000;

pub(super) fn body(req: Request<'_>) -> Value {
    // Consecutive user-side entries (tool results, then maybe text) must share one message.
    let mut messages: Vec<Value> = Vec::new();
    for message in req.messages {
        match message {
            Message::User { text } => {
                push_user_block(&mut messages, json!({ "type": "text", "text": text }));
            }
            Message::Tool {
                call_id,
                output,
                is_error,
                ..
            } => push_user_block(
                &mut messages,
                json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": output,
                    "is_error": is_error,
                }),
            ),
            Message::Assistant { text, calls, raw } => {
                let content = match raw {
                    Some(Value::Array(blocks)) if !blocks.is_empty() => {
                        Value::Array(blocks.clone())
                    }
                    _ => {
                        let mut blocks = Vec::new();
                        if !text.is_empty() {
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                        for c in calls {
                            blocks.push(json!({
                                "type": "tool_use", "id": c.id, "name": c.name, "input": input_object(&c.args),
                            }));
                        }
                        if blocks.is_empty() {
                            blocks.push(json!({ "type": "text", "text": "(no output)" }));
                        }
                        Value::Array(blocks)
                    }
                };
                messages.push(json!({ "role": "assistant", "content": content }));
            }
        }
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": MAX_TOKENS,
        "system": req.system,
        "messages": messages,
        "stream": true,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": tool_schema(&t.schema) }))
            .collect();
    }
    body
}

/// Add a block to the last message when it is a user message, else start a new user message.
fn push_user_block(messages: &mut Vec<Value>, block: Value) {
    if let Some(last) = messages.last_mut()
        && last["role"] == "user"
        && let Some(content) = last["content"].as_array_mut()
    {
        content.push(block);
        return;
    }
    messages.push(json!({ "role": "user", "content": [block] }));
}

/// `tool_use.input` must be an object; broken arguments become `{}`.
fn input_object(args: &Value) -> Value {
    if args.is_object() {
        args.clone()
    } else {
        json!({})
    }
}

/// Rebuilds content blocks from stream events.
#[derive(Default)]
pub(super) struct Accumulator {
    blocks: Vec<Value>,
    /// Partial JSON of each `tool_use` block's input, by block index.
    partial: Vec<String>,
    stop: String,
}

impl Accumulator {
    /// Apply one event's data; returns new visible text, if any.
    pub(super) fn apply(&mut self, data: &Value) -> Result<Option<String>, ModelError> {
        let index = data["index"].as_u64().unwrap_or(0) as usize;
        match data["type"].as_str().unwrap_or_default() {
            "content_block_start" => {
                while self.blocks.len() <= index {
                    self.blocks.push(Value::Null);
                    self.partial.push(String::new());
                }
                self.blocks[index] = data["content_block"].clone();
            }
            "content_block_delta" => {
                let Some(block) = self.blocks.get_mut(index) else {
                    return Ok(None);
                };
                let delta = &data["delta"];
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        append(block, "text", text);
                        return Ok(Some(text.to_string()));
                    }
                    "input_json_delta" => {
                        self.partial[index]
                            .push_str(delta["partial_json"].as_str().unwrap_or_default());
                    }
                    "thinking_delta" => {
                        append(
                            block,
                            "thinking",
                            delta["thinking"].as_str().unwrap_or_default(),
                        );
                    }
                    "signature_delta" => {
                        block["signature"] = delta["signature"].clone();
                    }
                    _ => {} // citations etc.: not needed here
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.blocks.get_mut(index)
                    && block["type"] == "tool_use"
                {
                    block["input"] = input_object(&parse_args(&self.partial[index]));
                }
            }
            "message_delta" => {
                if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                    self.stop = reason.to_string();
                }
            }
            "error" => {
                return Err(ModelError::Protocol(format!(
                    "stream error: {}",
                    data["error"]
                )));
            }
            _ => {} // message_start, message_stop, ping
        }
        Ok(None)
    }

    pub(super) fn finish(self) -> Reply {
        let blocks: Vec<Value> = self.blocks.into_iter().filter(|b| !b.is_null()).collect();
        let text = blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
        let calls = blocks
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                name: b["name"].as_str().unwrap_or_default().to_string(),
                args: b["input"].clone(),
            })
            .collect();
        Reply {
            text,
            calls,
            raw: Some(Value::Array(blocks)),
            stop: self.stop,
        }
    }
}

fn append(block: &mut Value, field: &str, text: &str) {
    let old = block[field].as_str().unwrap_or_default();
    block[field] = Value::String(format!("{old}{text}"));
}

fn request(client: &Client, http: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let http = http.header("anthropic-version", API_VERSION);
    match &client.api_key {
        Some(key) => http.header("x-api-key", key),
        None => http,
    }
}

pub(super) async fn complete(
    client: &Client,
    req: Request<'_>,
    on_text: OnText<'_>,
) -> Result<Reply, ModelError> {
    let http = client
        .http
        .post(format!("{}/v1/messages", client.base_url))
        .json(&body(req));
    let resp = check_status(request(client, http).send().await?).await?;
    let mut acc = Accumulator::default();
    for_each_event(resp, |event| {
        if event.data.is_empty() {
            return Ok(true);
        }
        let data: Value = serde_json::from_str(&event.data)
            .map_err(|e| ModelError::Protocol(format!("bad SSE data: {e}: {}", event.data)))?;
        if let Some(text) = acc.apply(&data)? {
            on_text(&text);
        }
        Ok(data["type"] != "message_stop")
    })
    .await?;
    Ok(acc.finish())
}

pub(super) async fn list_models(client: &Client) -> Result<Vec<String>, ModelError> {
    let http = client
        .http
        .get(format!("{}/v1/models?limit=100", client.base_url));
    let value: Value = check_status(request(client, http).send().await?)
        .await?
        .json()
        .await?;
    Ok(value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect())
}
