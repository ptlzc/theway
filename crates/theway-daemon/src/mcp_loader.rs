//! MCP server configuration loader. Reads `~/.theway/mcp.toml` (and `<cwd>/.theway/mcp.toml`),
//! spawns each configured stdio server, runs the initialize+tools/list handshake, and
//! returns the resulting AgentTool list ready to append to the session tool set.
//!
//! Failure is non-fatal at the load level: a server that fails to start emits a startup
//! diagnostic and is skipped. The agent runs without it.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use theway_core::AgentTool;
use theway_mcp::{
    HttpMcpAuth, HttpMcpTransport, HttpMcpTransportOptions, McpClient, ReconnectPolicy,
    StdioTransport,
};

use crate::triggers::McpNotificationHook;
use theway_daemon::tools::mcp_adapter::McpAgentTool;
use theway_transport::auth::AuthStore;
use theway_transport::client::base_dir;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct McpConfig {
    #[serde(default)]
    pub server: Vec<ServerConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub name: String,
    #[serde(default)]
    pub kind: ServerKind,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub endpoint: Option<String>,
    pub auth: Option<HttpAuthConfig>,
    pub request_timeout_ms: Option<u64>,
    pub sse_idle_timeout_ms: Option<u64>,
    pub body_cap_bytes: Option<usize>,
    pub reconnect: Option<ReconnectConfig>,
    /// Treat this server as a pure notification feed: its pushed `payload_summary` is
    /// injected straight into the parent chat (no sub-agent, no model call) instead of
    /// dispatching the dynamic-rule sub-agent. Off by default. See
    /// `triggers::direct_inject_action_hook`.
    #[serde(default)]
    pub inject_summary: bool,
    /// Like `inject_summary`, but additionally run ONE model turn in the parent's full
    /// context so the agent reacts to the notification. Off by default; wins over
    /// `inject_summary` if both are set. Authority note: this lets a trusted source's data
    /// wake the main agent (with tools + history) — opt in per server only.
    #[serde(default)]
    pub inject_and_run: bool,
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    #[default]
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HttpAuthConfig {
    pub kind: String,
    pub token_keychain_ref: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReconnectConfig {
    pub initial_ms: Option<u64>,
    pub max_ms: Option<u64>,
    pub max_attempts: Option<usize>,
}

/// Output of loading. Holds tools (to register with the agent), diagnostics (startup
/// failures to print to the user), and notification hooks (one per MCP server that
/// successfully connected — the caller is expected to register each with
/// `AgentHarness::register_notification_hook` once the harness is built so MCP server
/// pushes drive the runtime trigger pipeline).
pub struct LoadedMcp {
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub diagnostics: Vec<String>,
    pub client_count: usize,
    pub server_names: Vec<String>,
    pub notification_hooks: Vec<Arc<McpNotificationHook>>,
    /// Names of servers configured with `inject_summary = true`. The caller wires these into
    /// `triggers::direct_inject_action_hook` so their pushes bypass the sub-agent.
    pub inject_summary_servers: std::collections::HashSet<String>,
    /// Names of servers configured with `inject_and_run = true` — injected summary plus one
    /// model turn in the parent context.
    pub inject_and_run_servers: std::collections::HashSet<String>,
}

impl LoadedMcp {
    /// Empty load result — the issue #73 seam for startup without local
    /// `mcp.toml` scanning: when `StartupConfig::load_local_sources` is
    /// disabled the composition root uses this instead of [`load_all`].
    /// TODO(#73): controller-provisioned MCP servers arrive through the
    /// settings RPC in a later phase.
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            diagnostics: Vec::new(),
            client_count: 0,
            server_names: Vec::new(),
            notification_hooks: Vec::new(),
            inject_summary_servers: std::collections::HashSet::new(),
            inject_and_run_servers: std::collections::HashSet::new(),
        }
    }
}

/// Load and connect every MCP server from the project + user configs. Project entries with
/// the same `name` as a user entry override.
pub async fn load_all(cwd: &Path) -> LoadedMcp {
    let mut diagnostics = Vec::new();
    let project_path = cwd.join(".theway").join("mcp.toml");
    let user_path = base_dir().join("mcp.toml");

    let mut configs: Vec<ServerConfig> = Vec::new();
    for (path, label) in [(&user_path, "user"), (&project_path, "project")] {
        if let Some(cfg) = read_config(path, &mut diagnostics, label).await {
            for s in cfg.server {
                if let Some(i) = configs.iter().position(|x| x.name == s.name) {
                    configs[i] = s;
                } else {
                    configs.push(s);
                }
            }
        }
    }
    let inject_summary_servers: std::collections::HashSet<String> = configs
        .iter()
        .filter(|c| c.inject_summary)
        .map(|c| c.name.clone())
        .collect();
    let inject_and_run_servers: std::collections::HashSet<String> = configs
        .iter()
        .filter(|c| c.inject_and_run)
        .map(|c| c.name.clone())
        .collect();

    let (tools, notification_hooks, connect_diagnostics, client_count, server_names) =
        connect_all(&configs).await;
    diagnostics.extend(connect_diagnostics);
    LoadedMcp {
        tools,
        diagnostics,
        client_count,
        server_names,
        notification_hooks,
        inject_summary_servers,
        inject_and_run_servers,
    }
}

/// Connect to each configured server. Returns the tools collected, the
/// `McpNotificationHook` per successful connection, per-server failure diagnostics, and
/// the number of servers that actually connected.
///
/// `client_count` reports **successful** connections, not attempted ones. The TUI startup
/// banner prints "connected to N server(s)" using this field; previously it reported
/// `configs.len()`, so the user saw "connected to 3" alongside two error diagnostics when
/// 2 of 3 servers failed to start. See code-review item #9 (2026-05-22).
async fn connect_all(
    configs: &[ServerConfig],
) -> (
    Vec<Arc<dyn AgentTool>>,
    Vec<Arc<McpNotificationHook>>,
    Vec<String>,
    usize,
    Vec<String>,
) {
    let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
    let mut notification_hooks: Vec<Arc<McpNotificationHook>> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut client_count = 0usize;
    let mut server_names = Vec::new();
    for s in configs.iter() {
        match connect_one(s).await {
            Ok((server_tools, hook)) => {
                tools.extend(server_tools);
                notification_hooks.push(hook);
                client_count += 1;
                server_names.push(s.name.clone());
            }
            Err(e) => {
                diagnostics.push(format!("mcp server '{}' failed: {e}", s.name));
            }
        }
    }
    (
        tools,
        notification_hooks,
        diagnostics,
        client_count,
        server_names,
    )
}

async fn read_config(path: &Path, diagnostics: &mut Vec<String>, label: &str) -> Option<McpConfig> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return None;
    }
    match tokio::fs::read_to_string(path).await {
        Ok(text) => match toml::from_str::<McpConfig>(&text) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                diagnostics.push(format!(
                    "mcp config ({label}, {}): parse failed: {e}",
                    path.display()
                ));
                None
            }
        },
        Err(e) => {
            diagnostics.push(format!(
                "mcp config ({label}, {}): read failed: {e}",
                path.display()
            ));
            None
        }
    }
}

async fn connect_one(
    s: &ServerConfig,
) -> Result<(Vec<Arc<dyn AgentTool>>, Arc<McpNotificationHook>)> {
    let client = match s.kind {
        ServerKind::Stdio => connect_stdio(s).await?,
        ServerKind::StreamableHttp => connect_streamable_http(s).await?,
    };
    client.initialize("theway").await?;
    // Take the server-push notification receiver before any other consumer can claim it.
    // `take_notifications` returns `Some` exactly once per client; subsequent callers (and
    // an unconsumed channel for a long-running session) would silently buffer frames, so
    // the only correct moment is here, immediately after `initialize`. If the receiver is
    // already taken something invariant has been violated — we fail spawn rather than
    // silently disconnect the trigger surface.
    let rx = client.take_notifications().ok_or_else(|| {
        anyhow::anyhow!("McpClient::take_notifications returned None — receiver already consumed")
    })?;
    let hook = Arc::new(McpNotificationHook::new(s.name.clone(), rx));

    let tools = client.tools_list().await?;
    let mut out: Vec<Arc<dyn AgentTool>> = Vec::with_capacity(tools.len());
    for tool in &tools {
        let adapter = McpAgentTool::new(client.clone(), tool);
        out.push(Arc::new(adapter));
    }
    Ok((out, hook))
}

async fn connect_stdio(s: &ServerConfig) -> Result<Arc<McpClient>> {
    if s.endpoint.is_some() || s.auth.is_some() {
        anyhow::bail!(
            "stdio MCP server '{}' must not set endpoint or auth; remove streamable_http fields",
            s.name
        );
    }
    let command = s
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("stdio MCP server '{}' missing command", s.name))?;
    let args: Vec<&str> = s.args.iter().map(String::as_str).collect();
    let transport = StdioTransport::spawn(command, &args).await?;
    let client = Arc::new(McpClient::new(Arc::new(transport)));
    Ok(client)
}

async fn connect_streamable_http(s: &ServerConfig) -> Result<Arc<McpClient>> {
    if s.command.is_some() || !s.args.is_empty() {
        anyhow::bail!(
            "streamable_http MCP server '{}' must set endpoint, not command/args",
            s.name
        );
    }
    let endpoint = s
        .endpoint
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("streamable_http MCP server '{}' missing endpoint", s.name))?
        .clone();
    let mut opts = HttpMcpTransportOptions::new(endpoint);
    opts.auth = resolve_http_auth(s.auth.as_ref())?;
    if let Some(ms) = s.request_timeout_ms {
        if ms == 0 {
            anyhow::bail!(
                "streamable_http MCP server '{}' request_timeout_ms must be positive",
                s.name
            );
        }
        opts.request_timeout = std::time::Duration::from_millis(ms);
    }
    if let Some(ms) = s.sse_idle_timeout_ms {
        if ms == 0 {
            anyhow::bail!(
                "streamable_http MCP server '{}' sse_idle_timeout_ms must be positive",
                s.name
            );
        }
        opts.sse_idle_timeout = std::time::Duration::from_millis(ms);
    }
    if let Some(cap) = s.body_cap_bytes {
        if cap == 0 {
            anyhow::bail!(
                "streamable_http MCP server '{}' body_cap_bytes must be positive",
                s.name
            );
        }
        opts.body_cap_bytes = cap;
    }
    if let Some(reconnect) = &s.reconnect {
        if reconnect.initial_ms == Some(0) || reconnect.max_ms == Some(0) {
            anyhow::bail!(
                "streamable_http MCP server '{}' reconnect delays must be positive",
                s.name
            );
        }
        opts.reconnect_policy = ReconnectPolicy {
            initial_delay: std::time::Duration::from_millis(reconnect.initial_ms.unwrap_or(500)),
            max_delay: std::time::Duration::from_millis(reconnect.max_ms.unwrap_or(30_000)),
            max_attempts: reconnect.max_attempts,
        };
    }
    let transport = HttpMcpTransport::connect(opts)?;
    Ok(Arc::new(McpClient::new(Arc::new(transport))))
}

fn resolve_http_auth(auth: Option<&HttpAuthConfig>) -> Result<HttpMcpAuth> {
    let Some(auth_cfg) = auth else {
        return Ok(HttpMcpAuth::None);
    };
    let recovery = http_auth_recovery(auth_cfg);
    let store = AuthStore::load()
        .map_err(|e| anyhow::anyhow!("failed to load local credential store: {e}; {recovery}"))?;
    resolve_http_auth_from_store(Some(auth_cfg), &store)
}

fn resolve_http_auth_from_store(
    auth: Option<&HttpAuthConfig>,
    store: &AuthStore,
) -> Result<HttpMcpAuth> {
    let Some(auth) = auth else {
        return Ok(HttpMcpAuth::None);
    };
    if auth.kind != "bearer" {
        anyhow::bail!("unsupported streamable_http auth kind; expected bearer");
    }
    let token_ref = auth
        .token_keychain_ref
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("bearer auth requires token_keychain_ref"))?;
    let recovery = http_auth_recovery(auth);
    let token = store
        .resolve_for_provider(token_ref)
        .ok_or_else(|| anyhow::anyhow!("configured bearer credential was not found; {recovery}"))?;
    Ok(HttpMcpAuth::Bearer { token })
}

fn http_auth_recovery(auth: &HttpAuthConfig) -> &'static str {
    let _ = auth;
    "run /login <configured-token-ref>"
}

#[cfg(test)]
// Test files live in `tests/mcp_loader/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("mcp_loader");
