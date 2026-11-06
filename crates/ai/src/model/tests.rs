//! Adapter round trips against mock HTTP servers (no real model calls).

use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request as HttpRequest, ResponseTemplate};

use super::*;

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

fn sse_named(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect()
}

fn stream(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "tabs_open".into(),
        description: "Open a tab.".into(),
        schema: json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "OpenArgs",
            "type": "object",
            "properties": { "url": { "type": "string" } },
        }),
    }]
}

async fn complete(client: &Client, messages: &[Message]) -> (Reply, String) {
    let tools = tools();
    let mut streamed = String::new();
    let mut on_text = |t: &str| streamed.push_str(t);
    let reply = client
        .complete(
            Request {
                model: "m",
                system: "sys",
                messages,
                tools: &tools,
            },
            &mut on_text,
        )
        .await
        .unwrap();
    (reply, streamed)
}

fn body_of(req: &HttpRequest) -> Value {
    serde_json::from_slice(&req.body).unwrap()
}

#[tokio::test]
async fn openai_tool_call_round_trip() {
    let server = MockServer::start().await;
    let first = sse(&[
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": "Open"}}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "ing"}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "tabs_open", "arguments": "{\"url\":"}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"https://a.test/\"}"}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
    ]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(stream(first))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(
        Api::OpenAi,
        format!("{}/v1/", server.uri()),
        Some("sk-test".into()),
    );
    let mut messages = vec![Message::User {
        text: "open a.test".into(),
    }];
    let (reply, streamed) = complete(&client, &messages).await;
    assert_eq!(streamed, "Opening");
    assert_eq!(reply.text, "Opening");
    assert_eq!(reply.stop, "tool_calls");
    assert_eq!(
        reply.calls,
        vec![ToolCall {
            id: "call_1".into(),
            name: "tabs_open".into(),
            args: json!({"url": "https://a.test/"})
        }]
    );

    // The request: system first, tools as functions without `$schema`, streaming on.
    let sent = body_of(&server.received_requests().await.unwrap()[0]);
    assert_eq!(sent["stream"], true);
    assert_eq!(
        sent["messages"][0],
        json!({"role": "system", "content": "sys"})
    );
    assert_eq!(sent["tools"][0]["function"]["name"], "tabs_open");
    assert!(
        sent["tools"][0]["function"]["parameters"]
            .get("$schema")
            .is_none()
    );

    // Second turn: the tool result goes back as role "tool" next to the assistant tool_calls.
    messages.push(reply.into_message());
    messages.push(Message::Tool {
        call_id: "call_1".into(),
        name: "tabs_open".into(),
        output: "{\"tab\":2}".into(),
        is_error: false,
    });
    let body = openai::body(Request {
        model: "m",
        system: "sys",
        messages: &messages,
        tools: &[],
    });
    assert_eq!(
        body["messages"][2]["tool_calls"][0]["function"]["arguments"],
        "{\"url\":\"https://a.test/\"}"
    );
    assert_eq!(
        body["messages"][3],
        json!({"role": "tool", "tool_call_id": "call_1", "content": "{\"tab\":2}"})
    );
    assert!(body.get("tools").is_none());
}

#[tokio::test]
async fn openai_non_streamed_answer_and_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ok/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "hi", "tool_calls": [
                {"id": "c9", "type": "function", "function": {"name": "tabs_open", "arguments": "{}"}}
            ]}, "finish_reason": "tool_calls"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/bad/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("{\"error\":\"bad key\"}"))
        .mount(&server)
        .await;

    let client = Client::new(Api::OpenAi, format!("{}/ok", server.uri()), None);
    let (reply, streamed) = complete(&client, &[Message::User { text: "x".into() }]).await;
    assert_eq!((reply.text.as_str(), streamed.as_str()), ("hi", "hi"));
    assert_eq!(reply.calls[0].id, "c9");

    let client = Client::new(Api::OpenAi, format!("{}/bad", server.uri()), None);
    let tools = tools();
    let err = client
        .complete(
            Request {
                model: "m",
                system: "",
                messages: &[],
                tools: &tools,
            },
            &mut |_: &str| {},
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ModelError::Http {
            status: 401,
            body: "{\"error\":\"bad key\"}".into()
        }
    );
}

#[tokio::test]
async fn anthropic_tool_use_round_trip() {
    let server = MockServer::start().await;
    let body = sse_named(&[
        json!({"type": "message_start", "message": {"id": "msg_1", "role": "assistant", "content": []}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "need a tab"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig=="}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Sure."}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "tabs_open", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"url\": \"https://"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "a.test/\"}"}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 9}}),
        json!({"type": "message_stop"}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(stream(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(Api::Anthropic, server.uri(), Some("sk-ant".into()));
    let mut messages = vec![Message::User {
        text: "open a.test".into(),
    }];
    let (reply, streamed) = complete(&client, &messages).await;
    assert_eq!(streamed, "Sure.");
    assert_eq!(reply.stop, "tool_use");
    assert_eq!(reply.calls[0].args, json!({"url": "https://a.test/"}));
    let raw = reply.raw.clone().unwrap();
    assert_eq!(
        raw[0],
        json!({"type": "thinking", "thinking": "need a tab", "signature": "sig=="})
    );

    let sent = body_of(&server.received_requests().await.unwrap()[0]);
    assert_eq!(sent["system"], "sys");
    assert_eq!(sent["tools"][0]["input_schema"]["type"], "object");
    assert!(sent["tools"][0]["input_schema"].get("$schema").is_none());

    // Next request: thinking block replayed unchanged; tool results merged into one user message.
    messages.push(reply.into_message());
    for (id, out) in [("toolu_1", "{\"tab\":2}"), ("toolu_x", "{}")] {
        messages.push(Message::Tool {
            call_id: id.into(),
            name: "tabs_open".into(),
            output: out.into(),
            is_error: false,
        });
    }
    let body = anthropic::body(Request {
        model: "m",
        system: "s",
        messages: &messages,
        tools: &[],
    });
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1]["content"][0]["signature"], "sig==");
    assert_eq!(
        msgs[1]["content"][2]["input"],
        json!({"url": "https://a.test/"})
    );
    assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
}

#[tokio::test]
async fn anthropic_stream_error_event() {
    let server = MockServer::start().await;
    let body = sse_named(&[
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(stream(body))
        .mount(&server)
        .await;
    let client = Client::new(Api::Anthropic, server.uri(), Some("k".into()));
    let err = client
        .complete(
            Request {
                model: "m",
                system: "",
                messages: &[],
                tools: &[],
            },
            &mut |_: &str| {},
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ModelError::Protocol(m) if m.contains("Overloaded")));
}

#[tokio::test]
async fn gemini_function_call_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-test:generateContent"))
        .and(header("x-goog-api-key", "g-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"text": "thinking...", "thought": true},
                    {"text": "Opening it."},
                    {"functionCall": {"name": "tabs_open", "args": {"url": "https://a.test/"}}, "thoughtSignature": "ts1"}
                ]},
                "finishReason": "STOP"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::new(Api::Gemini, server.uri(), Some("g-key".into()));
    let tools = tools();
    let mut messages = vec![Message::User {
        text: "open".into(),
    }];
    let mut streamed = String::new();
    let reply = client
        .complete(
            Request {
                model: "gemini-test",
                system: "sys",
                messages: &messages,
                tools: &tools,
            },
            &mut |t: &str| streamed.push_str(t),
        )
        .await
        .unwrap();
    assert_eq!(reply.text, "Opening it.");
    assert_eq!(streamed, "Opening it.");
    assert_eq!(reply.calls[0].name, "tabs_open");
    assert_eq!(reply.calls[0].args["url"], "https://a.test/");

    let sent = body_of(&server.received_requests().await.unwrap()[0]);
    assert_eq!(sent["systemInstruction"]["parts"][0]["text"], "sys");
    let decl = &sent["tools"][0]["functionDeclarations"][0];
    assert_eq!(decl["name"], "tabs_open");
    assert!(decl["parametersJsonSchema"].get("$schema").is_none());

    // Next request: model parts replayed with the thought signature, then a functionResponse.
    messages.push(reply.into_message());
    messages.push(Message::Tool {
        call_id: "call_2".into(),
        name: "tabs_open".into(),
        output: "{\"tab\":3}".into(),
        is_error: false,
    });
    let body = gemini::body(Request {
        model: "gemini-test",
        system: "s",
        messages: &messages,
        tools: &[],
    });
    assert_eq!(body["contents"][1]["role"], "model");
    assert_eq!(body["contents"][1]["parts"][2]["thoughtSignature"], "ts1");
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"],
        json!({"name": "tabs_open", "response": {"result": {"tab": 3}}})
    );
}

#[tokio::test]
async fn list_models_per_api() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "a"}, {"id": "b"}]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"name": "models/g1", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]},
        ]})))
        .mount(&server)
        .await;
    let openai = Client::new(Api::OpenAi, format!("{}/v1", server.uri()), None);
    assert_eq!(openai.list_models().await.unwrap(), vec!["a", "b"]);
    let anthropic = Client::new(Api::Anthropic, server.uri(), Some("k".into()));
    assert_eq!(anthropic.list_models().await.unwrap(), vec!["a", "b"]);
    let gemini = Client::new(Api::Gemini, server.uri(), Some("k".into()));
    assert_eq!(gemini.list_models().await.unwrap(), vec!["g1"]);
}

#[test]
fn truncate_marks_cut_text() {
    assert_eq!(truncate("abc", 5), "abc");
    assert!(truncate("abcdef", 3).starts_with("abc… [truncated, 6 chars total]"));
}
