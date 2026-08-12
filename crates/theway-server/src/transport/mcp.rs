//! MCP server transport — expose the theway local-execution tool set over the Model
//! Context Protocol (stdio JSON-RPC 2.0).
//!
//! Complements `theway-mcp` (the *client* that calls external MCP servers): in `--mcp`
//! mode the theway process *is* an MCP server, so any MCP client (Claude Code, Codex,
//! IDEs, other agents) can call theway's local tools (bash / fs / git / web / exec
//! group) as standard MCP tools over stdio. `initialize` / `ping` are handled by the
//! server loop; this module provides the tool surface and the `tools/call` dispatch.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult};
use theway_llm_provider::UserContentBlock;
use theway_mcp::{McpDispatcher, McpError};
use tokio_util::sync::CancellationToken;

/// Run the MCP stdio server exposing `tools`. Blocks until stdin closes.
pub async fn run_mcp_server(tools: Vec<Arc<dyn AgentTool>>) -> Result<(), McpError> {
    let dispatcher = ToolDispatcher { tools };
    theway_mcp::run_stdio_server(&dispatcher).await
}

/// McpDispatcher backed by an `AgentTool` list.
struct ToolDispatcher {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl ToolDispatcher {
    fn find(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.tools.iter().find(|t| t.definition().name == name)
    }

    fn mcp_tool_schema(&self, tool: &dyn AgentTool) -> Value {
        let def = tool.definition();
        json!({
            "name": def.name,
            "description": def.description,
            "inputSchema": def.parameters,
        })
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

#[async_trait]
impl McpDispatcher for ToolDispatcher {
    async fn handle(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "theway", "version": env!("CARGO_PKG_VERSION") },
            })),
            "tools/list" => {
                let tools: Vec<Value> = self
                    .tools
                    .iter()
                    .map(|t| self.mcp_tool_schema(t.as_ref()))
                    .collect();
                Ok(json!({ "tools": tools }))
            }
            "tools/call" => {
                let p = params
                    .ok_or_else(|| McpError::Protocol("tools/call requires params".to_string()))?;
                let name = p
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::Protocol("tools/call missing `name`".to_string()))?;
                let arguments = p
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Default::default()));
                let tool = self
                    .find(name)
                    .ok_or_else(|| McpError::Protocol(format!("tool not found: {name}")))?;
                match tool
                    .execute("mcp", arguments, CancellationToken::new(), None)
                    .await
                {
                    Ok(result) => Ok(json!({
                        "content": [{ "type": "text", "text": render_tool_result(&result) }],
                        "isError": false,
                    })),
                    // Tool execution failures are tool results with isError, not RPC errors.
                    Err(AgentToolError::Message(msg)) => Ok(json!({
                        "content": [{ "type": "text", "text": msg }],
                        "isError": true,
                    })),
                    Err(e) => Ok(json!({
                        "content": [{ "type": "text", "text": e.to_string() }],
                        "isError": true,
                    })),
                }
            }
            _ => Err(McpError::Protocol(format!("method not found: {method}"))),
        }
    }
}
