//! Additional tests for `mcp_adapter` — kept in a separate bridged module so the
//! original inline suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use std::sync::Arc;
use theway_core::{AgentTool, ToolExecutionMode};
use theway_mcp::Transport;
use theway_mcp::protocol::McpTool;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;

struct PipeTransport {
    tx: AsyncMutex<mpsc::UnboundedSender<String>>,
    rx: AsyncMutex<mpsc::UnboundedReceiver<String>>,
}

#[async_trait::async_trait]
impl Transport for PipeTransport {
    async fn send_line(&self, line: String) -> Result<(), theway_mcp::errors::McpError> {
        self.tx
            .lock()
            .await
            .send(line)
            .map_err(|e| theway_mcp::errors::McpError::Transport(e.to_string()))
    }
    async fn recv_line(&self) -> Result<Option<String>, theway_mcp::errors::McpError> {
        Ok(self.rx.lock().await.recv().await)
    }
    async fn close(&self) {}
}

fn pair() -> (Arc<PipeTransport>, Arc<PipeTransport>) {
    let (a_tx, b_rx) = mpsc::unbounded_channel();
    let (b_tx, a_rx) = mpsc::unbounded_channel();
    (
        Arc::new(PipeTransport {
            tx: AsyncMutex::new(a_tx),
            rx: AsyncMutex::new(a_rx),
        }),
        Arc::new(PipeTransport {
            tx: AsyncMutex::new(b_tx),
            rx: AsyncMutex::new(b_rx),
        }),
    )
}

/// EOF transport: the read pump exits immediately. Good for construction-only
/// tests and for the `NotInitialized` error path.
struct EofTransport;

#[async_trait::async_trait]
impl Transport for EofTransport {
    async fn send_line(&self, _line: String) -> Result<(), theway_mcp::errors::McpError> {
        Ok(())
    }
    async fn recv_line(&self) -> Result<Option<String>, theway_mcp::errors::McpError> {
        Ok(None)
    }
    async fn close(&self) {}
}

#[derive(Clone)]
enum ToolsCallResponse {
    Result(serde_json::Value),
    Error { code: i64, message: &'static str },
}

/// Mock MCP server: answers `initialize`, ignores notifications, and answers
/// `tools/call` with the supplied response.
async fn run_server(transport: Arc<PipeTransport>, tools_call: ToolsCallResponse) {
    while let Ok(Some(line)) = transport.recv_line().await {
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").and_then(|i| i.as_u64());
        match method {
            "initialize" => {
                let _ = transport
                    .send_line(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": "2025-03-26",
                                "capabilities": {},
                                "serverInfo": { "name": "mock", "version": "0.0.1" }
                            }
                        })
                        .to_string(),
                    )
                    .await;
            }
            "tools/call" => {
                let response = match tools_call.clone() {
                    ToolsCallResponse::Result(result) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    }),
                    ToolsCallResponse::Error { code, message } => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": code, "message": message },
                    }),
                };
                let _ = transport.send_line(response.to_string()).await;
            }
            _ => {}
        }
    }
}

fn mcp_tool() -> McpTool {
    McpTool {
        name: "mock_tool".into(),
        description: Some("mock description".into()),
        input_schema: serde_json::json!({ "type": "object" }),
    }
}

fn tool_for(client: Arc<theway_mcp::McpClient>) -> McpAgentTool {
    McpAgentTool::new(client, &mcp_tool())
}

#[tokio::test]
async fn definition_label_and_execution_mode_are_forwarded() {
    let client = Arc::new(theway_mcp::McpClient::new(Arc::new(EofTransport)));
    let tool = tool_for(client);

    assert_eq!(tool.definition().name, "mock_tool");
    assert_eq!(tool.definition().description, "mock description");
    assert_eq!(
        tool.definition().parameters,
        serde_json::json!({ "type": "object" })
    );
    assert_eq!(tool.label(), "mock_tool");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));
}

#[tokio::test]
async fn execute_maps_text_image_and_resource_blocks() {
    let (client_side, server_side) = pair();
    tokio::spawn(run_server(
        server_side,
        ToolsCallResponse::Result(serde_json::json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "image", "data": "aW1n", "mimeType": "image/png" },
                { "type": "resource", "resource": { "uri": "file:///x", "text": "r" } }
            ],
            "isError": false
        })),
    ));

    let client = Arc::new(theway_mcp::McpClient::new(client_side));
    client.initialize("theway-test").await.unwrap();
    let tool = tool_for(client);

    let result = tool
        .execute(
            "call-1",
            serde_json::json!({}),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("successful MCP call should map to a tool result");

    assert_eq!(result.details["name"], "mock_tool");
    assert_eq!(result.details["isError"], false);
    assert_eq!(result.content.len(), 3);
    match &result.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => assert_eq!(t.text, "hello"),
        other => panic!("expected text block, got {other:?}"),
    }
    match &result.content[1] {
        theway_llm_provider::UserContentBlock::Image(img) => {
            assert_eq!(img.data, "aW1n");
            assert_eq!(img.mime_type, "image/png");
        }
        other => panic!("expected image block, got {other:?}"),
    }
    match &result.content[2] {
        theway_llm_provider::UserContentBlock::Text(t) => {
            assert!(t.text.contains("<resource>"), "got: {}", t.text);
            assert!(t.text.contains("file:///x"), "got: {}", t.text);
        }
        other => panic!("expected text resource block, got {other:?}"),
    }
}

#[tokio::test]
async fn execute_maps_is_error_content_to_agent_tool_error() {
    let (client_side, server_side) = pair();
    tokio::spawn(run_server(
        server_side,
        ToolsCallResponse::Result(serde_json::json!({
            "content": [ { "type": "text", "text": "boom" } ],
            "isError": true
        })),
    ));

    let client = Arc::new(theway_mcp::McpClient::new(client_side));
    client.initialize("theway-test").await.unwrap();
    let tool = tool_for(client);

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({}),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("isError must surface as an AgentToolError");

    match err {
        theway_core::AgentToolError::Message(m) => assert_eq!(m, "boom"),
        other => panic!("expected Message error, got {other}"),
    }
}

#[tokio::test]
async fn execute_maps_mcp_server_error_to_agent_tool_error() {
    let (client_side, server_side) = pair();
    tokio::spawn(run_server(
        server_side,
        ToolsCallResponse::Error {
            code: -32000,
            message: "kaboom",
        },
    ));

    let client = Arc::new(theway_mcp::McpClient::new(client_side));
    client.initialize("theway-test").await.unwrap();
    let tool = tool_for(client);

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({}),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("MCP server errors must surface as AgentToolError");

    let msg = err.to_string();
    assert!(
        msg.contains("mcp call: server returned error -32000: kaboom"),
        "got: {msg}"
    );
}

#[tokio::test]
async fn execute_maps_not_initialized_error() {
    let client = Arc::new(theway_mcp::McpClient::new(Arc::new(EofTransport)));
    let tool = tool_for(client);

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({}),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("calling before initialize must fail");

    let msg = err.to_string();
    assert!(
        msg.contains("mcp call: client is not initialized"),
        "got: {msg}"
    );
}
