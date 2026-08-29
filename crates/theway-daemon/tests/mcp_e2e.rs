//! MCP server end-to-end test: spawn `thewayd --mcp` and exercise it through the
//! theway-mcp client (full initialize + initialized handshake), verifying the
//! stdio JSON-RPC surface: initialize, tools/list (the 15 local-execution tools),
//! tools/call (bash executes), and unknown-tool error.

use serde_json::json;
use tempfile::TempDir;
use theway_mcp::{McpClient, StdioTransport};

fn thewayd_bin() -> &'static str {
    env!("CARGO_BIN_EXE_thewayd")
}

#[tokio::test]
async fn mcp_server_initialize_tools_list_and_call() {
    // Run the MCP server from an empty scratch cwd/home so local source
    // discovery (skills, MCP clients, hooks) does not scan the repository and
    // stall startup.
    let cwd = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let transport = Arc::new(
        StdioTransport::spawn(
            thewayd_bin(),
            &[
                "--mcp",
                "--cwd",
                cwd.path().to_str().unwrap(),
                "--home",
                home.path().to_str().unwrap(),
                "--theway-dir",
                data.path().to_str().unwrap(),
            ],
        )
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
