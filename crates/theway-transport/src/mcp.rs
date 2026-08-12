//! MCP server transport — expose the theway local-execution tool set over the Model
//! Context Protocol (stdio JSON-RPC 2.0), using the [`rmcp`] SDK (the industry-standard
//! Rust MCP implementation) instead of a hand-written JSON-RPC loop.
//!
//! In `--mcp` mode the theway process *is* an MCP server: any MCP client (Claude Code,
//! Codex, IDEs, other agents) can call theway's local tools (bash / fs / git / web /
//! exec group) as standard MCP tools over stdio. `initialize` / `ping` / protocol
//! negotiation come from `rmcp`; this module supplies the tool surface and dispatch.

use std::sync::Arc;

use rmcp::model::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerInfo, TextContent, Tool,
};
use rmcp::service::{RequestContext, RoleServer, serve_server};
use rmcp::{ServerHandler, transport::io::stdio};
use serde_json::Value;
use theway_core::{AgentTool, AgentToolError, AgentToolResult};
use theway_llm_provider::UserContentBlock;
use tokio_util::sync::CancellationToken;

/// Run the MCP stdio server exposing `tools`. Blocks until stdin closes.
pub async fn run_mcp_server(tools: Vec<Arc<dyn AgentTool>>) -> anyhow::Result<()> {
    let dispatcher = ToolDispatcher { tools };
    let (stdin, stdout) = stdio();
    let service = serve_server(dispatcher, (stdin, stdout)).await?;
    service.waiting().await?;
    Ok(())
}

/// `ServerHandler` backed by an `AgentTool` list.
struct ToolDispatcher {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl ToolDispatcher {
    fn find(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.tools.iter().find(|t| t.definition().name == name)
    }

    fn mcp_tool(&self, tool: &dyn AgentTool) -> Tool {
        let def = tool.definition();
        let schema: Arc<rmcp::model::JsonObject> =
            Arc::new(serde_json::from_value(def.parameters.clone()).unwrap_or_default());
        Tool::new(def.name.clone(), def.description.clone(), schema)
    }
}

/// Render an `AgentToolResult` as MCP text content (text blocks joined; structured
/// `details` appended as JSON when present).
fn render_tool_result(result: &AgentToolResult) -> String {
    let mut text = String::new();
    for block in &result.content {
        match block {
            UserContentBlock::Text(t) => {
                text.push_str(&t.text);
                if !t.text.ends_with('\n') {
                    text.push('\n');
                }
            }
            other => {
                text.push_str(&format!("[{other:?}]\n"));
            }
        }
    }
    if result.details != Value::Object(Default::default()) {
        text.push_str(&format!("{}\n", result.details));
    }
    text
}

impl ServerHandler for ToolDispatcher {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
            .with_server_info(Implementation::new("theway", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self
                .tools
                .iter()
                .map(|t| self.mcp_tool(t.as_ref()))
                .collect(),
            next_cursor: None,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = &request.name;
        let arguments = request
            .arguments
            .map(|a| serde_json::to_value(a).unwrap_or_default())
            .unwrap_or_else(|| Value::Object(Default::default()));
        let tool = self.find(name).ok_or_else(|| {
            McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("tool not found: {name}"),
                None,
            )
        })?;
        match tool
            .execute(name, arguments, CancellationToken::new(), None)
            .await
        {
            Ok(result) => Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::Text(TextContent::new(render_tool_result(&result))),
            ]))),
            // Tool execution failures are tool results with isError, not RPC errors.
            Err(AgentToolError::Message(msg)) => {
                Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                    ContentBlock::Text(TextContent::new(msg)),
                ])))
            }
            Err(e) => Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                ContentBlock::Text(TextContent::new(e.to_string())),
            ]))),
        }
    }
}
