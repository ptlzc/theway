//! Session-level MCP layering (session-scoped-mcp): a session server with the
//! same name as a daemon server replaces it instead of being connected twice.

use super::*;

use std::collections::HashSet;

use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::Tool;
use theway_transport::wire::{
    WireProvisionedMcpAuth, WireProvisionedMcpReconnect, WireProvisionedMcpServer,
};
use tokio_util::sync::CancellationToken;

struct StubTool {
    definition: Tool,
}

fn stub_tool(name: &str) -> Arc<dyn AgentTool> {
    Arc::new(StubTool {
        definition: Tool {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    })
}

#[async_trait::async_trait]
impl AgentTool for StubTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        &self.definition.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Err(AgentToolError::Message("stub tool".into()))
    }
}

fn connected_server(name: &str, tool: &str) -> ConnectedMcpServer {
    ConnectedMcpServer {
        name: name.into(),
        tools: vec![stub_tool(tool)],
        hook: hook_for(name),
    }
}

fn layer(
    servers: Vec<ConnectedMcpServer>,
    inject_summary: &[&str],
    inject_and_run: &[&str],
    errors: &[(&str, &str)],
) -> McpLayer {
    McpLayer {
        servers,
        inject_summary: inject_summary.iter().map(|name| name.to_string()).collect(),
        inject_and_run: inject_and_run.iter().map(|name| name.to_string()).collect(),
        errors: errors
            .iter()
            .map(|(name, message)| (name.to_string(), message.to_string()))
            .collect(),
    }
}

#[test]
fn merge_mcp_layers_replaces_daemon_server_with_same_name() {
    // Arrange: "shared" exists on both layers; each layer also has one unique server.
    let daemon = layer(
        vec![
            connected_server("shared", "daemon_shared_tool"),
            connected_server("daemon-only", "daemon_only_tool"),
        ],
        &["shared"],
        &[],
        &[("shared", "old daemon failure")],
    );
    let overlay = layer(
        vec![
            connected_server("shared", "session_shared_tool"),
            connected_server("session-only", "session_only_tool"),
        ],
        &["session-only"],
        &["shared"],
        &[],
    );
    let overlay_names: HashSet<String> = ["shared".to_string(), "session-only".to_string()]
        .into_iter()
        .collect();

    // Act
    let merged = merge_mcp_layers(&daemon, &overlay_names, &overlay);

    // Assert: the daemon "shared" entry is gone; the session one took its slot.
    assert_eq!(
        merged.server_names(),
        vec![
            "daemon-only".to_string(),
            "shared".to_string(),
            "session-only".to_string()
        ]
    );
    assert_eq!(
        merged.tool_names(),
        vec![
            "daemon_only_tool".to_string(),
            "session_shared_tool".to_string(),
            "session_only_tool".to_string()
        ]
    );
    assert_eq!(merged.hooks().len(), 3);
    assert!(
        !merged.inject_summary.contains("shared"),
        "the daemon inject flag must go with the replaced server"
    );
    assert!(merged.inject_summary.contains("session-only"));
    assert!(merged.inject_and_run.contains("shared"));
    assert!(
        merged.errors.is_empty(),
        "the daemon failure row for the replaced server must not survive"
    );
}

#[test]
fn merge_mcp_layers_shadows_daemon_server_when_session_connect_fails() {
    // Arrange: the session "shared" server never came up (no connected server).
    let daemon = layer(
        vec![connected_server("shared", "daemon_shared_tool")],
        &["shared"],
        &[],
        &[],
    );
    let overlay = layer(
        Vec::new(),
        &[],
        &[],
        &[("shared", "spawn failed")],
    );
    let overlay_names: HashSet<String> = ["shared".to_string()].into_iter().collect();

    // Act
    let merged = merge_mcp_layers(&daemon, &overlay_names, &overlay);

    // Assert: no silent fallback to the daemon server.
    assert!(merged.servers.is_empty());
    assert!(merged.tools().is_empty());
    assert!(!merged.inject_summary.contains("shared"));
    assert_eq!(
        merged.errors,
        vec![("shared".to_string(), "spawn failed".to_string())]
    );
}

#[test]
fn merge_mcp_layers_without_overlay_keeps_daemon_layer() {
    // Arrange
    let daemon = layer(
        vec![connected_server("daemon-only", "daemon_only_tool")],
        &["daemon-only"],
        &[],
        &[],
    );

    // Act
    let merged = merge_mcp_layers(&daemon, &HashSet::new(), &McpLayer::default());

    // Assert
    assert_eq!(merged.server_names(), vec!["daemon-only".to_string()]);
    assert_eq!(merged.tool_names(), vec!["daemon_only_tool".to_string()]);
    assert_eq!(merged.inject_summary, daemon.inject_summary);
    assert_eq!(merged.hooks().len(), 1);
}

#[test]
fn server_config_from_wire_maps_stdio_and_streamable_http() {
    // Arrange
    let stdio = WireProvisionedMcpServer {
        name: "fs".into(),
        kind: "stdio".into(),
        command: Some("/usr/bin/mcp-fs".into()),
        args: vec!["--root".into(), "/srv".into()],
        request_timeout_ms: Some(5000),
        body_cap_bytes: Some(1024),
        reconnect: Some(WireProvisionedMcpReconnect {
            initial_ms: Some(10),
            max_ms: Some(20),
            max_attempts: Some(3),
        }),
        inject_summary: true,
        ..Default::default()
    };
    let remote = WireProvisionedMcpServer {
        name: "remote".into(),
        kind: "streamable_http".into(),
        endpoint: Some("https://mcp.example.com".into()),
        auth: Some(WireProvisionedMcpAuth {
            kind: "bearer".into(),
            token_keychain_ref: Some("mcp/remote".into()),
        }),
        inject_and_run: true,
        ..Default::default()
    };

    // Act
    let stdio = server_config_from_wire(&stdio);
    let remote = server_config_from_wire(&remote);

    // Assert
    assert_eq!(stdio.name, "fs");
    assert_eq!(stdio.kind, ServerKind::Stdio);
    assert_eq!(stdio.command.as_deref(), Some("/usr/bin/mcp-fs"));
    assert_eq!(stdio.args, vec!["--root", "/srv"]);
    assert_eq!(stdio.request_timeout_ms, Some(5000));
    assert_eq!(stdio.body_cap_bytes, Some(1024));
    assert!(stdio.inject_summary);
    let reconnect = stdio.reconnect.as_ref().unwrap();
    assert_eq!(reconnect.initial_ms, Some(10));
    assert_eq!(reconnect.max_attempts, Some(3));

    assert_eq!(remote.kind, ServerKind::StreamableHttp);
    assert_eq!(
        remote.endpoint.as_deref(),
        Some("https://mcp.example.com")
    );
    let auth = remote.auth.as_ref().unwrap();
    assert_eq!(auth.kind, "bearer");
    assert_eq!(auth.token_keychain_ref.as_deref(), Some("mcp/remote"));
    assert!(remote.inject_and_run);
}
