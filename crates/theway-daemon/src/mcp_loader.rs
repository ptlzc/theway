//! MCP server configuration loader. Reads `paths.base/mcp.toml` (and
//! `paths.work_dir/.theway/mcp.toml`), spawns each configured stdio server in
//! `paths.work_dir`, runs the initialize+tools/list handshake, and returns the
//! resulting AgentTool list ready to append to the session tool set.
//!
//! Failure is non-fatal at the load level: a server that fails to start emits a startup
//! diagnostic and is skipped. The agent runs without it.

use std::collections::HashSet;
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
use theway_transport::wire::WireProvisionedMcpServer;

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct McpConfig {
    #[serde(default)]
    pub server: Vec<ServerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    #[default]
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpAuthConfig {
    pub kind: String,
    pub token_keychain_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReconnectConfig {
    pub initial_ms: Option<u64>,
    pub max_ms: Option<u64>,
    pub max_attempts: Option<usize>,
}

/// One successfully connected MCP server: the config name it was connected
/// under, the tools it exposed, and its notification hook. Grouping tools and
/// hook under the server name is what lets a session-level layer replace a
/// daemon-level server without reconnecting anything.
#[derive(Clone)]
pub struct ConnectedMcpServer {
    pub name: String,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub hook: Arc<McpNotificationHook>,
}

/// One MCP layer: connected servers plus the inject policy and per-server
/// failures that belong with them. The daemon layer comes from `mcp.toml` or
/// the settings `Configure` path; the session layer comes from
/// `ActivateSession.mcp_servers`.
#[derive(Clone, Default)]
pub struct McpLayer {
    pub servers: Vec<ConnectedMcpServer>,
    pub inject_summary: HashSet<String>,
    pub inject_and_run: HashSet<String>,
    /// Per-server failures as `(name, message)`.
    pub errors: Vec<(String, String)>,
}

impl McpLayer {
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }

    pub fn hooks(&self) -> Vec<Arc<McpNotificationHook>> {
        self.servers
            .iter()
            .map(|server| server.hook.clone())
            .collect()
    }

    pub fn server_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .map(|server| server.name.clone())
            .collect()
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools()
            .iter()
            .map(|tool| tool.definition().name.clone())
            .collect()
    }
}

/// Layer `overlay` over `daemon`: an overlay server name replaces the daemon
/// entry with the same name (tools, hook, inject flags, failure row) so one
/// session never runs two servers under one name. `overlay_names` comes from
/// the requested configs, not from successful connections — a session server
/// that failed to connect still shadows the daemon entry instead of silently
/// falling back to it.
pub fn merge_mcp_layers(
    daemon: &McpLayer,
    overlay_names: &HashSet<String>,
    overlay: &McpLayer,
) -> McpLayer {
    let mut servers: Vec<ConnectedMcpServer> = daemon
        .servers
        .iter()
        .filter(|server| !overlay_names.contains(&server.name))
        .cloned()
        .collect();
    servers.extend(overlay.servers.iter().cloned());
    let mut inject_summary = daemon.inject_summary.clone();
    inject_summary.retain(|name| !overlay_names.contains(name));
    inject_summary.extend(overlay.inject_summary.iter().cloned());
    let mut inject_and_run = daemon.inject_and_run.clone();
    inject_and_run.retain(|name| !overlay_names.contains(name));
    inject_and_run.extend(overlay.inject_and_run.iter().cloned());
    let mut errors: Vec<(String, String)> = daemon
        .errors
        .iter()
        .filter(|(name, _)| !overlay_names.contains(name))
        .cloned()
        .collect();
    errors.extend(overlay.errors.iter().cloned());
    McpLayer {
        servers,
        inject_summary,
        inject_and_run,
        errors,
    }
}

/// Convert one wire-provisioned server (settings `Configure` or
/// `ActivateSession.mcp_servers`) into the loader's config shape.
pub(crate) fn server_config_from_wire(server: &WireProvisionedMcpServer) -> ServerConfig {
    ServerConfig {
        name: server.name.clone(),
        kind: match server.kind.as_str() {
            "streamable_http" => ServerKind::StreamableHttp,
            _ => ServerKind::Stdio,
        },
        command: server.command.clone(),
        args: server.args.clone(),
        endpoint: server.endpoint.clone(),
        auth: server.auth.as_ref().map(|auth| HttpAuthConfig {
            kind: auth.kind.clone(),
            token_keychain_ref: auth.token_keychain_ref.clone(),
        }),
        request_timeout_ms: server.request_timeout_ms,
        sse_idle_timeout_ms: server.sse_idle_timeout_ms,
        body_cap_bytes: server.body_cap_bytes.map(|bytes| bytes as usize),
        reconnect: server.reconnect.as_ref().map(|reconnect| ReconnectConfig {
            initial_ms: reconnect.initial_ms,
            max_ms: reconnect.max_ms,
            max_attempts: reconnect.max_attempts.map(|attempts| attempts as usize),
        }),
        inject_summary: server.inject_summary,
        inject_and_run: server.inject_and_run,
    }
}

/// Output of loading. Holds the applied configs, the connected-server layer
/// (tools + notification hooks + inject policy + failures), and diagnostics
/// (startup failures to print to the user). Hooks are one per successfully
/// connected server — the caller registers each with the harness once it is
/// built so MCP server pushes drive the runtime trigger pipeline.
pub struct LoadedMcp {
    /// Configs applied in connect order (project entries override user ones).
    pub configs: Vec<ServerConfig>,
    /// Connected servers with their inject policy and per-server failures.
    pub layer: McpLayer,
    pub diagnostics: Vec<String>,
}

impl LoadedMcp {
    /// Empty load result — the issue #73 seam for startup without local
    /// `mcp.toml` scanning: when `StartupConfig::load_local_sources` is
    /// disabled the composition root uses this instead of [`load_all`].
    /// Controller-provisioned servers arrive through the settings RPC and
    /// the [`McpProvisionState`] slot below.
    pub fn empty() -> Self {
        Self {
            configs: Vec::new(),
            layer: McpLayer::default(),
            diagnostics: Vec::new(),
        }
    }
}

/// Load and connect every MCP server from the project + user configs. Project entries with
/// the same `name` as a user entry override.
pub async fn load_all(paths: &theway_daemon::DaemonPaths) -> LoadedMcp {
    let mut diagnostics = Vec::new();
    let project_path = paths.work_dir.join(".theway").join("mcp.toml");
    let user_path = paths.base.join("mcp.toml");

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

    let (layer, connect_diagnostics) =
        connect_servers(&configs, &paths.work_dir, &paths.base.join("auth.json")).await;
    diagnostics.extend(connect_diagnostics);
    LoadedMcp {
        configs,
        layer,
        diagnostics,
    }
}

/// Connect to each configured server and return the resulting layer plus the
/// raw per-server failure diagnostics.
///
/// `layer.servers` reports **successful** connections, not attempted ones: the TUI
/// startup banner prints "connected to N server(s)" from it, and a server that
/// failed to start contributes an error row instead.
pub(crate) async fn connect_servers(
    configs: &[ServerConfig],
    cwd: &Path,
    auth_path: &Path,
) -> (McpLayer, Vec<String>) {
    let mut servers: Vec<ConnectedMcpServer> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    for s in configs.iter() {
        match connect_one(s, cwd, auth_path).await {
            Ok((tools, hook)) => servers.push(ConnectedMcpServer {
                name: s.name.clone(),
                tools,
                hook,
            }),
            Err(e) => {
                diagnostics.push(format!("mcp server '{}' failed: {e}", s.name));
            }
        }
    }
    let layer = McpLayer {
        servers,
        inject_summary: configs
            .iter()
            .filter(|config| config.inject_summary)
            .map(|config| config.name.clone())
            .collect(),
        inject_and_run: configs
            .iter()
            .filter(|config| config.inject_and_run)
            .map(|config| config.name.clone())
            .collect(),
        errors: diagnostics
            .iter()
            .map(|diagnostic| parse_mcp_diagnostic(diagnostic))
            .collect(),
    };
    (layer, diagnostics)
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
    cwd: &Path,
    auth_path: &Path,
) -> Result<(Vec<Arc<dyn AgentTool>>, Arc<McpNotificationHook>)> {
    let client = match s.kind {
        ServerKind::Stdio => connect_stdio(s, cwd).await?,
        ServerKind::StreamableHttp => connect_streamable_http(s, auth_path).await?,
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

async fn connect_stdio(s: &ServerConfig, cwd: &Path) -> Result<Arc<McpClient>> {
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
    let transport = StdioTransport::spawn_in(command, &args, cwd).await?;
    let client = Arc::new(McpClient::new(Arc::new(transport)));
    Ok(client)
}

async fn connect_streamable_http(s: &ServerConfig, auth_path: &Path) -> Result<Arc<McpClient>> {
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
    opts.auth = resolve_http_auth(s.auth.as_ref(), auth_path)?;
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

fn resolve_http_auth(auth: Option<&HttpAuthConfig>, auth_path: &Path) -> Result<HttpMcpAuth> {
    let Some(auth_cfg) = auth else {
        return Ok(HttpMcpAuth::None);
    };
    let recovery = http_auth_recovery(auth_cfg);
    let store = AuthStore::load_from(auth_path)
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

/// Runtime slot for controller-provisioned MCP servers (issue #73): the
/// settings `Configure` path connects the servers and stores the result
/// here. Session builds read tools/hooks/inject sets from this slot in
/// controller mode; `/reload` reconnects from the stored configs.
#[derive(Default)]
pub struct McpProvisionState {
    /// The last applied server configs — reconnect source for `/reload`.
    pub configs: Vec<ServerConfig>,
    /// Connected servers, grouped by name so a session-level layer can replace
    /// a same-name daemon server without reconnecting anything.
    pub servers: Vec<ConnectedMcpServer>,
    /// Tools from the currently connected servers (flat view of `servers`).
    pub tools: Vec<Arc<dyn AgentTool>>,
    /// Notification hooks (one per connected server).
    pub hooks: Vec<Arc<McpNotificationHook>>,
    /// Names of connected servers configured with `inject_summary = true`.
    pub inject_summary: std::collections::HashSet<String>,
    /// Names of connected servers configured with `inject_and_run = true`.
    pub inject_and_run: std::collections::HashSet<String>,
    /// Successfully connected server names, in config order.
    pub server_names: Vec<String>,
    /// Tool names from the connected servers.
    pub tool_names: Vec<String>,
    /// Per-server failures as `(name, message)` — flows into
    /// `WireMcpSnapshot.errors` so the TUI shows the 3s banner and red
    /// panel rows.
    pub errors: Vec<(String, String)>,
    /// `mcp:<name>` labels of hooks already registered on the live
    /// session's trigger executor. Reset on every (re)connection — the
    /// new hook instances replace the old ones.
    pub registered_labels: std::collections::HashSet<String>,
}

impl McpProvisionState {
    /// Replace the state from a fresh connection result. `registered_labels`
    /// resets because the hooks are new instances.
    pub(crate) fn replace_connection_result(
        &mut self,
        configs: Vec<ServerConfig>,
        result: (McpLayer, Vec<String>),
    ) {
        let (layer, _diagnostics) = result;
        self.configs = configs;
        self.servers = layer.servers;
        self.inject_summary = layer.inject_summary;
        self.inject_and_run = layer.inject_and_run;
        self.errors = layer.errors;
        self.server_names = self.servers.iter().map(|s| s.name.clone()).collect();
        self.tools = self
            .servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect();
        self.hooks = self
            .servers
            .iter()
            .map(|server| server.hook.clone())
            .collect();
        self.tool_names = self
            .tools
            .iter()
            .map(|tool| tool.definition().name.clone())
            .collect();
        self.registered_labels.clear();
    }

    /// The connected-server layer, for merging a session-level overlay over it.
    pub fn layer(&self) -> McpLayer {
        McpLayer {
            servers: self.servers.clone(),
            inject_summary: self.inject_summary.clone(),
            inject_and_run: self.inject_and_run.clone(),
            errors: self.errors.clone(),
        }
    }

    /// Mark the current hooks as registered on the owning session executor.
    /// A per-session slot installed at activation is followed by exactly one
    /// build that registers these hooks, so the labels can be seeded up front
    /// and a later `Configure` re-merge only registers genuinely new hooks.
    pub(crate) fn mark_hooks_registered(&mut self) {
        use crate::trigger_engine::notification_hook::NotificationHook;
        for hook in &self.hooks {
            let label = hook.label().to_string();
            self.registered_labels.insert(label);
        }
    }
}

/// Split one loader diagnostic into a `(name, message)` pair. Server
/// failures carry the server name; config-file problems carry the file
/// label. Any unrecognized diagnostic keeps the full text under `mcp`.
///
/// Defined here (not in `orchestration/session/resources`) so the
/// path-included integration tests can reach it; the session-resource model
/// re-exports it for its own namespace.
pub fn parse_mcp_diagnostic(diagnostic: &str) -> (String, String) {
    if let Some(rest) = diagnostic.strip_prefix("mcp server '") {
        if let Some((name, message)) = rest.split_once("' failed: ") {
            return (name.to_string(), message.to_string());
        }
    }
    if let Some(rest) = diagnostic.strip_prefix("mcp config (") {
        if let Some((head, message)) = rest.split_once("): ") {
            // `head` = `user, /path/to/mcp.toml` — the label is the segment
            // before the comma; the message follows `): `.
            let label = head.split_once(", ").map_or(head, |(l, _)| l);
            return (format!("mcp.toml ({label})"), message.to_string());
        }
    }
    ("mcp".to_string(), diagnostic.to_string())
}

/// Validate a provisioned server list: every entry needs a non-empty name
/// and names must be unique within the list.
pub(crate) fn validate_unique_names(configs: &[ServerConfig]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for config in configs {
        if config.name.trim().is_empty() {
            return Err("mcp_servers: server name must not be empty".to_string());
        }
        if !seen.insert(config.name.as_str()) {
            return Err(format!(
                "mcp_servers: duplicate server name '{}'",
                config.name
            ));
        }
    }
    Ok(())
}
