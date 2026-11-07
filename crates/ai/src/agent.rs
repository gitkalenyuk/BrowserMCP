//! The agent loop for HTTP model APIs (Anthropic, OpenAI-compatible, Gemini).
//!
//! For every user message: send the conversation + tool definitions to the model; run the tool
//! calls it asks for through `Registry::call` as `Caller::Agent { id }`; send the results back;
//! repeat until the model answers without tool calls (or `max_steps` is reached).

use std::sync::{Arc, Weak};

use bmcp_core::{BrowserBackend, Caller, Ctx, EventBus, Registry, Toolset};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::model::{Client, Message, Reply, Request, ToolCall, ToolDef, truncate};
use crate::session::{AgentEvent, Session, Status, summarize};

/// Longest tool output (chars) sent back to the model.
pub const MAX_TOOL_OUTPUT: usize = 30_000;

/// The system prompt for API agents.
pub fn system_prompt(agent: &str) -> String {
    format!(
        "You are an AI agent built into BrowserMCP, a real Chromium browser, with agent id \
         {agent}. Act through the tools. See pages with page_snapshot (compact text with refs \
         like @3:e5), find elements with page_find, act with page_act using a ref; actions \
         return a diff. Take page_screenshot only if text is not enough. Address tabs by the ids \
         from tabs_list; tabs you open or act on join your colored tab group. Tool errors come \
         back as JSON {{code, message, retry, hint}}: follow the hint. Be brief in replies."
    )
}

/// Tool definitions for the given toolsets (wire names + argument schemas from the registry).
pub fn tool_defs(registry: &Registry, toolsets: &[Toolset]) -> Vec<ToolDef> {
    registry
        .list(Some(toolsets))
        .into_iter()
        .map(|info| ToolDef {
            name: info.wire_name.clone(),
            description: info.description.to_string(),
            schema: info.args_schema.clone(),
        })
        .collect()
}

/// Runs tool calls for one agent.
#[derive(Clone)]
pub struct ToolRunner {
    pub registry: Weak<Registry>,
    pub backend: Arc<dyn BrowserBackend>,
    pub events: EventBus,
    pub agent: String,
}

/// The result of one tool call.
pub struct ToolOutcome {
    pub ok: bool,
    /// Output or error, as JSON.
    pub value: Value,
}

impl ToolRunner {
    pub async fn run(&self, call: &ToolCall) -> ToolOutcome {
        let Some(registry) = self.registry.upgrade() else {
            return ToolOutcome {
                ok: false,
                value: json!({ "code": "SHUTTING_DOWN", "message": "the browser is shutting down" }),
            };
        };
        if !call.args.is_object() {
            return ToolOutcome {
                ok: false,
                value: json!({
                    "code": "INVALID_PARAMS",
                    "message": format!("arguments of {} are not a JSON object: {}", call.name, call.args),
                    "retry": "no",
                }),
            };
        }
        let ctx = Ctx::new(
            Caller::Agent {
                id: self.agent.clone(),
            },
            self.backend.clone(),
            self.events.clone(),
        );
        match registry.call(ctx, &call.name, call.args.clone()).await {
            Ok(output) => ToolOutcome {
                ok: true,
                value: output,
            },
            Err(err) => ToolOutcome {
                ok: false,
                value: serde_json::to_value(&err).unwrap_or_default(),
            },
        }
    }
}

/// Output as text for the model: images dropped (only text adapters for now), long text cut.
pub fn output_text(value: &Value) -> String {
    let mut value = value.clone();
    let is_image = value["mime_type"]
        .as_str()
        .is_some_and(|m| m.starts_with("image/"));
    if is_image && let Some(data) = value.get("data").and_then(Value::as_str) {
        value["data"] = json!(format!("[image omitted, {} base64 chars]", data.len()));
    }
    truncate(&value.to_string(), MAX_TOOL_OUTPUT)
}

/// Add the next user message. A history that already ends with one (a chat stopped before
/// the model answered, or restored from a transcript) gets it appended, so the conversation
/// keeps alternating roles, as the providers expect.
pub fn push_user(messages: &mut Vec<Message>, text: String) {
    match messages.last_mut() {
        Some(Message::User { text: last }) => {
            last.push_str("\n\n");
            last.push_str(&text);
        }
        _ => messages.push(Message::User { text }),
    }
}

/// Everything one API session needs.
pub struct ApiAgent {
    pub session: Arc<Session>,
    pub client: Client,
    pub model: String,
    pub tools: Vec<ToolDef>,
    pub runner: ToolRunner,
    pub max_steps: u32,
}

impl ApiAgent {
    /// The session task: handle user messages until stopped. The conversation starts from the
    /// session's kept history, so a stopped chat continues where it was.
    pub async fn run(self, mut input: mpsc::UnboundedReceiver<String>) {
        let session = self.session.clone();
        let cancel = session.cancel_token();
        let system = system_prompt(&session.id);
        let mut messages: Vec<Message> = session.api_history();
        loop {
            let text = tokio::select! {
                _ = cancel.cancelled() => break,
                text = input.recv() => match text {
                    Some(text) => text,
                    None => break,
                },
            };
            push_user(&mut messages, text);
            session.set_status(Status::Running);
            let reason = tokio::select! {
                _ = cancel.cancelled() => "stopped".to_string(),
                reason = self.answer(&system, &mut messages) => reason,
            };
            // Kept (and repaired: a stop mid-step leaves tool calls without results) for the
            // next message, and for continuing after a stop or a browser restart.
            session.set_api_history(messages.clone());
            messages = session.api_history();
            session.apply(AgentEvent::Done(reason));
            if cancel.is_cancelled() {
                session.set_status(Status::Stopped);
                return;
            }
            session.set_status(Status::Idle);
        }
        // Stopped before the queued message was picked up: still end that answer.
        if session.status() == Status::Running {
            session.apply(AgentEvent::Done("stopped".into()));
        }
        session.set_status(Status::Stopped);
    }

    /// Model ↔ tools until the model stops calling tools. Returns the `ai.done` reason.
    async fn answer(&self, system: &str, messages: &mut Vec<Message>) -> String {
        for _ in 0..self.max_steps {
            let reply = match self.step(system, messages).await {
                Ok(reply) => reply,
                Err(err) => {
                    let message = format!("error: {err}");
                    self.session.apply(AgentEvent::Notice(message.clone()));
                    return message;
                }
            };
            if !reply.text.is_empty() {
                self.session.apply(AgentEvent::Text(reply.text.clone()));
            }
            let calls = reply.calls.clone();
            messages.push(reply.into_message());
            if calls.is_empty() {
                return "end_turn".into();
            }
            for call in &calls {
                self.session.apply(AgentEvent::ToolCall {
                    name: call.name.clone(),
                    args: call.args.clone(),
                });
                let outcome = self.runner.run(call).await;
                let output = output_text(&outcome.value);
                self.session.apply(AgentEvent::ToolResult {
                    name: call.name.clone(),
                    ok: outcome.ok,
                    summary: summarize(&outcome.value),
                    output: Some(outcome.value),
                });
                messages.push(Message::Tool {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    output,
                    is_error: !outcome.ok,
                });
            }
        }
        "max_steps".into()
    }

    async fn step(
        &self,
        system: &str,
        messages: &[Message],
    ) -> Result<Reply, crate::model::ModelError> {
        let session = self.session.clone();
        let mut on_text =
            move |delta: &str| session.apply(AgentEvent::TextDelta(delta.to_string()));
        let request = Request {
            model: &self.model,
            system,
            messages,
            tools: &self.tools,
        };
        self.client.complete(request, &mut on_text).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_messages_never_follow_each_other() {
        let mut messages = Vec::new();
        push_user(&mut messages, "a".into());
        push_user(&mut messages, "b".into());
        assert_eq!(
            messages,
            vec![Message::User {
                text: "a\n\nb".into()
            }]
        );
        messages.push(Message::Assistant {
            text: "ok".into(),
            calls: vec![],
            raw: None,
        });
        push_user(&mut messages, "c".into());
        assert_eq!(messages.len(), 3);
    }
}
