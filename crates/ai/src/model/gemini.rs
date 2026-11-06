//! Google Gemini, native `generateContent` with function calling (one JSON response per turn).
//!
//! `POST {base}/v1beta/models/{model}:generateContent`, key in `x-goog-api-key`. Tools are
//! `functionDeclarations` with `parametersJsonSchema` (full JSON Schema, so registry schemas
//! need no conversion). The model's `parts` are replayed unchanged, which keeps the
//! `thoughtSignature`s newer models require next to their function calls.

use serde_json::{Value, json};

use super::{
    Client, Message, ModelError, OnText, Reply, Request, ToolCall, check_status, tool_schema,
};

pub(super) fn body(req: Request<'_>) -> Value {
    let mut contents: Vec<Value> = Vec::new();
    for message in req.messages {
        match message {
            Message::User { text } => push_part(&mut contents, "user", json!({ "text": text })),
            Message::Tool { name, output, .. } => {
                // `response` must be an object: wrap the command output (or error) as `result`.
                let result = serde_json::from_str::<Value>(output).unwrap_or(json!(output));
                push_part(
                    &mut contents,
                    "user",
                    json!({ "functionResponse": { "name": name, "response": { "result": result } } }),
                );
            }
            Message::Assistant { text, calls, raw } => {
                let parts = match raw {
                    Some(Value::Array(parts)) if !parts.is_empty() => parts.clone(),
                    _ => {
                        let mut parts = Vec::new();
                        if !text.is_empty() {
                            parts.push(json!({ "text": text }));
                        }
                        for c in calls {
                            parts.push(
                                json!({ "functionCall": { "name": c.name, "args": c.args } }),
                            );
                        }
                        if parts.is_empty() {
                            parts.push(json!({ "text": "(no output)" }));
                        }
                        parts
                    }
                };
                contents.push(json!({ "role": "model", "parts": parts }));
            }
        }
    }
    let mut body = json!({
        "systemInstruction": { "parts": [{ "text": req.system }] },
        "contents": contents,
    });
    if !req.tools.is_empty() {
        let declarations: Vec<Value> = req
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "parametersJsonSchema": tool_schema(&t.schema) }))
            .collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    body
}

/// Append a part to the last content of `role`, or start a new content.
fn push_part(contents: &mut Vec<Value>, role: &str, part: Value) {
    if let Some(last) = contents.last_mut()
        && last["role"] == role
        && let Some(parts) = last["parts"].as_array_mut()
    {
        parts.push(part);
        return;
    }
    contents.push(json!({ "role": role, "parts": [part] }));
}

/// A `generateContent` response → reply.
pub(super) fn parse_response(value: &Value) -> Result<Reply, ModelError> {
    let Some(candidate) = value["candidates"].get(0) else {
        let reason = &value["promptFeedback"]["blockReason"];
        return Err(ModelError::Protocol(format!(
            "no candidates (blockReason: {reason})"
        )));
    };
    let parts = candidate["content"]["parts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if let Some(call) = part.get("functionCall") {
            calls.push(ToolCall {
                id: call["id"]
                    .as_str()
                    .map_or_else(|| format!("call_{i}"), str::to_string),
                name: call["name"].as_str().unwrap_or_default().to_string(),
                args: if call["args"].is_object() {
                    call["args"].clone()
                } else {
                    json!({})
                },
            });
        } else if part["thought"] != true
            && let Some(t) = part["text"].as_str()
        {
            text.push_str(t);
        }
    }
    Ok(Reply {
        text,
        calls,
        raw: Some(Value::Array(parts)),
        stop: candidate["finishReason"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

fn with_key(client: &Client, http: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match &client.api_key {
        Some(key) => http.header("x-goog-api-key", key),
        None => http,
    }
}

pub(super) async fn complete(
    client: &Client,
    req: Request<'_>,
    on_text: OnText<'_>,
) -> Result<Reply, ModelError> {
    let url = format!(
        "{}/v1beta/models/{}:generateContent",
        client.base_url, req.model
    );
    let http = with_key(client, client.http.post(url).json(&body(req)));
    let value: Value = check_status(http.send().await?).await?.json().await?;
    let reply = parse_response(&value)?;
    if !reply.text.is_empty() {
        on_text(&reply.text);
    }
    Ok(reply)
}

pub(super) async fn list_models(client: &Client) -> Result<Vec<String>, ModelError> {
    let url = format!("{}/v1beta/models?pageSize=200", client.base_url);
    let value: Value = check_status(with_key(client, client.http.get(url)).send().await?)
        .await?
        .json()
        .await?;
    Ok(value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_none_or(|methods| methods.iter().any(|x| x == "generateContent"))
        })
        .filter_map(|m| m["name"].as_str())
        .map(|name| name.trim_start_matches("models/").to_string())
        .collect())
}
