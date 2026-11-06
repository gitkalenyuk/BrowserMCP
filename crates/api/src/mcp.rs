//! MCP server generated from the command registry.
//!
//! Every registered command becomes one MCP tool (`tabs.list` → `tabs_list`, because the
//! Anthropic API rejects dots in tool names). Only some toolsets are listed by default so
//! tool schemas don't flood the model's context; clients pick more with
//! `http://127.0.0.1:47800/mcp?toolsets=core,net,record`.

use std::str::FromStr;
use std::sync::Arc;

use bmcp_core::{
    BrowserBackend, Caller, CommandInfo, Ctx, EventBus, Registry, Sensitivity, Toolset,
};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, model::*, service::RequestContext};
use serde_json::Value;

/// Toolsets listed when the client does not ask for specific ones.
pub const DEFAULT_TOOLSETS: &[Toolset] = &[Toolset::Core, Toolset::System];

#[derive(Clone)]
pub struct BrowserMcp {
    registry: Arc<Registry>,
    backend: Arc<dyn BrowserBackend>,
    events: EventBus,
}

impl BrowserMcp {
    pub fn new(
        registry: Arc<Registry>,
        backend: Arc<dyn BrowserBackend>,
        events: EventBus,
    ) -> Self {
        Self {
            registry,
            backend,
            events,
        }
    }
}

fn to_tool(info: &CommandInfo) -> Tool {
    let object = |v: &Value| Arc::new(v.as_object().cloned().unwrap_or_default());
    Tool::new(
        info.wire_name.clone(),
        info.description,
        object(&info.args_schema),
    )
    .with_raw_output_schema(object(&info.output_schema))
    .with_annotations(
        ToolAnnotations::new()
            .read_only(info.sensitivity == Sensitivity::Read)
            .destructive(false),
    )
}

fn http_parts(cx: &RequestContext<RoleServer>) -> Option<&axum::http::request::Parts> {
    cx.extensions.get::<axum::http::request::Parts>()
}

/// `?toolsets=core,net` from the request URL, else the defaults. Unknown names are ignored.
fn requested_toolsets(cx: &RequestContext<RoleServer>) -> Vec<Toolset> {
    let query = http_parts(cx).and_then(|p| p.uri.query()).unwrap_or("");
    let requested: Vec<Toolset> = query
        .split('&')
        .filter_map(|pair| pair.strip_prefix("toolsets="))
        .flat_map(|list| list.split(','))
        .filter_map(|name| Toolset::from_str(name.trim()).ok())
        .collect();
    if requested.is_empty() {
        DEFAULT_TOOLSETS.to_vec()
    } else {
        requested
    }
}

fn session_id(cx: &RequestContext<RoleServer>) -> String {
    http_parts(cx)
        .and_then(|p| p.headers.get("mcp-session-id"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned()
}

impl ServerHandler for BrowserMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("browsermcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "BrowserMCP controls a real Chromium browser. Read pages with page_ax_tree or \
                 page_text instead of screenshots. Tabs are addressed by the ids from tabs_list. \
                 Errors come back as JSON {code, message, retry, hint}."
                    .to_string(),
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        cx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let toolsets = requested_toolsets(&cx);
        let tools = self
            .registry
            .list(Some(&toolsets))
            .into_iter()
            .map(to_tool)
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        cx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let caller = Caller::Mcp {
            session: session_id(&cx),
            agent: None,
        };
        let ctx = Ctx::new(caller, self.backend.clone(), self.events.clone());
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let result = match self.registry.call(ctx, &request.name, args).await {
            Ok(output) => CallToolResult::structured(output),
            // {code, message, retry, hint, candidates}: readable by the model, not a protocol error.
            Err(err) => {
                CallToolResult::structured_error(serde_json::to_value(&err).unwrap_or_default())
            }
        };
        Ok(result.into())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.registry.get(name).map(to_tool)
    }
}
