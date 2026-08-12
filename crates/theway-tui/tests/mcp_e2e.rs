//! MCP server end-to-end test: spawn `theway --mcp` and exercise it through the
//! theway-mcp client (full initialize + initialized handshake), verifying the
//! stdio JSON-RPC surface: initialize, tools/list (the 15 local-execution tools),
//! tools/call (bash executes), and unknown-tool error.

use serde_json::json;
use theway_mcp::{McpClient, StdioTransport};

fn theway_bin() -> &'static str {
    env!("CARGO_BIN_EXE_theway")
}

#[tokio::test]
async fn mcp_server_initialize_tools_list_and_call() {
    let transport = Arc::new(
        StdioTransport::spawn(theway_bin(), &["--mcp"])
            .await
            .expect("spawn theway --mcp"),
    );
    let client = McpClient::new(transport);

    // initialize handshake (serverInfo + notifications/initialized).
    let info = client.initialize("e2e").await.expect("initialize");
    assert_eq!(info.server_info.name, "theway", "serverInfo name");

    // tools/list: the 15 local-execution tools.
    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for expected in ["read", "write", "bash", "exec", "grep", "git", "web_fetch"] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }

    // tools/call: bash executes a command.
    let result = client
        .tools_call(
            "bash",
            Some(json!({ "command": "echo mcp-e2e-works" })),
            None,
        )
        .await
        .expect("tools/call bash");
    let text: String = result
        .content
        .iter()
        .map(|c| match c {
            theway_mcp::protocol::ToolContent::Text { text } => text.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(
        text.contains("mcp-e2e-works"),
        "bash output missing marker: {text}"
    );

    // tools/call: unknown tool → error.
    let err = client
        .tools_call("definitely-not-a-tool", Some(json!({})), None)
        .await;
    assert!(err.is_err(), "unknown tool must error");

    client.close().await;
}

use std::sync::Arc;
