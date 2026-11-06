//! MCP tools (spike S3). Tool names use `_` because the Anthropic API rejects dots.
//!
//! Phase 3 replaces these hand-written tools with ones generated from the command
//! registry; the names below already follow the planned catalog.

use std::sync::Arc;
use std::time::Duration;

use bmcp_core::{BackendError, BrowserBackend, TabId};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Clone)]
pub struct BrowserMcp {
    backend: Arc<dyn BrowserBackend>,
    // Kept per the rmcp pattern; with rmcp 3.9 `#[tool_handler]` builds the router itself.
    #[allow(dead_code)]
    tool_router: ToolRouter<BrowserMcp>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenArgs {
    /// URL to open in a new tab.
    pub url: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GotoArgs {
    /// URL to load.
    pub url: String,
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<i32>,
    /// Max seconds to wait for the page to finish loading (default 30).
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TabArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<i32>,
    /// Maximum lines/characters to return.
    pub limit: Option<usize>,
}

fn ok_text(text: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        text.into(),
    )]))
}

/// Backend failures are tool errors the model can read and react to, not protocol errors.
fn tool_error(err: BackendError) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        err.to_string(),
    )]))
}

impl BrowserMcp {
    pub fn new(backend: Arc<dyn BrowserBackend>) -> Self {
        Self {
            backend,
            tool_router: Self::tool_router(),
        }
    }

    async fn resolve_tab(&self, tab: Option<i32>) -> Result<TabId, BackendError> {
        match tab {
            Some(id) => Ok(TabId(id)),
            None => self
                .backend
                .list_tabs()
                .await?
                .into_iter()
                .map(|t| t.id)
                .max()
                .ok_or(BackendError::NoTabs),
        }
    }
}

#[tool_router]
impl BrowserMcp {
    #[tool(description = "Check that BrowserMCP is running. Returns its version.")]
    async fn system_ping(&self) -> Result<CallToolResult, McpError> {
        ok_text(format!(
            "pong from BrowserMCP {}",
            env!("CARGO_PKG_VERSION")
        ))
    }

    #[tool(description = "List open tabs: id, title, url, loading.")]
    async fn tabs_list(&self) -> Result<CallToolResult, McpError> {
        match self.backend.list_tabs().await {
            Ok(tabs) => ok_text(
                tabs.iter()
                    .map(|t| {
                        let loading = if t.loading { " (loading)" } else { "" };
                        format!("tab {} \"{}\" {}{loading}", t.id, t.title, t.url)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Err(e) => tool_error(e),
        }
    }

    #[tool(description = "Open a URL in a new tab. Returns the new tab id.")]
    async fn tabs_open(
        &self,
        Parameters(args): Parameters<OpenArgs>,
    ) -> Result<CallToolResult, McpError> {
        match self.backend.open_tab(args.url).await {
            Ok(id) => ok_text(format!("opened tab {id}")),
            Err(e) => tool_error(e),
        }
    }

    #[tool(description = "Navigate a tab to a URL and wait until the page has loaded.")]
    async fn nav_goto(
        &self,
        Parameters(args): Parameters<GotoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let result = async {
            let tab = self.resolve_tab(args.tab).await?;
            let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(30));
            self.backend.navigate(tab, args.url, timeout).await
        }
        .await;
        match result {
            Ok(t) => ok_text(format!("tab {} loaded \"{}\" {}", t.id, t.title, t.url)),
            Err(e) => tool_error(e),
        }
    }

    #[tool(
        description = "Accessibility tree of the page as an indented list of roles and names. \
                       Use it to understand the page without a screenshot."
    )]
    async fn page_ax_tree(
        &self,
        Parameters(args): Parameters<TabArgs>,
    ) -> Result<CallToolResult, McpError> {
        let result = async {
            let tab = self.resolve_tab(args.tab).await?;
            self.backend
                .cdp(tab, "Accessibility.getFullAXTree".into(), json!({}))
                .await
        }
        .await;
        match result {
            Ok(tree) => ok_text(crate::ax::format_ax_tree(&tree, args.limit.unwrap_or(300))),
            Err(e) => tool_error(e),
        }
    }

    #[tool(description = "Visible text of the page (document.body.innerText).")]
    async fn page_text(
        &self,
        Parameters(args): Parameters<TabArgs>,
    ) -> Result<CallToolResult, McpError> {
        let limit = args.limit.unwrap_or(8000);
        let result = async {
            let tab = self.resolve_tab(args.tab).await?;
            self.backend
                .cdp(
                    tab,
                    "Runtime.evaluate".into(),
                    json!({ "expression": "document.body ? document.body.innerText : ''", "returnByValue": true }),
                )
                .await
        }
        .await;
        match result {
            Ok(v) => {
                let text = v["result"]["value"].as_str().unwrap_or_default();
                let clipped: String = text.chars().take(limit).collect();
                ok_text(clipped)
            }
            Err(e) => tool_error(e),
        }
    }
}

#[tool_handler]
impl ServerHandler for BrowserMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("browsermcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "BrowserMCP controls a real Chromium browser. Read pages with page_ax_tree or \
                 page_text instead of screenshots. Tabs are addressed by the ids from tabs_list."
                    .to_string(),
            )
    }
}
