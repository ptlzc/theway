//! MCP stdio server exposing daemon-owned agent tools through the `rmcp` SDK.

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
        self.tools
            .iter()
            .find(|tool| tool.definition().name == name)
    }

    fn mcp_tool(&self, tool: &dyn AgentTool) -> Tool {
        let definition = tool.definition();
        let schema: Arc<rmcp::model::JsonObject> =
            Arc::new(serde_json::from_value(definition.parameters.clone()).unwrap_or_default());
        Tool::new(
            definition.name.clone(),
            definition.description.clone(),
            schema,
        )
    }
}

fn render_tool_result(result: &AgentToolResult) -> String {
    let mut text = String::new();
    for block in &result.content {
        match block {
            UserContentBlock::Text(block) => {
                text.push_str(&block.text);
                if !block.text.ends_with('\n') {
                    text.push('\n');
                }
            }
            other => text.push_str(&format!("[{other:?}]\n")),
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
                .map(|tool| self.mcp_tool(tool.as_ref()))
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
            .map(|arguments| serde_json::to_value(arguments).unwrap_or_default())
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
            Err(AgentToolError::Message(message)) => {
                Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                    ContentBlock::Text(TextContent::new(message)),
                ])))
            }
            Err(error) => Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                ContentBlock::Text(TextContent::new(error.to_string())),
            ]))),
        }
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("mcp_server");
