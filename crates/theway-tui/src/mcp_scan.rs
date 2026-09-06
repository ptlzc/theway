//! Controller-side MCP server discovery (open spec provision-mcp-servers): the
//! TUI owns local `mcp.toml` scanning and provisions the daemon with the
//! scanned `[[server]]` catalog through the settings surface
//! (`WireDaemonConfig.mcp_servers`), so a controller-provisioned daemon never
//! reads MCP config files itself.
//!
//! Ordering and same-name-replacement semantics mirror the daemon's
//! `crate::mcp_loader::load_all`: roots are `base/mcp.toml` (user) then
//! `<cwd>/.theway/mcp.toml` (project), user entries are loaded first, and a
//! project entry replaces a user entry of the same name in place. A parse
//! failure of either file produces a `(file_label, error)` diagnostic and
//! yields the other file's entries.

use std::path::Path;

use serde::Deserialize;
use theway_transport::wire::{
    WireProvisionedMcpAuth, WireProvisionedMcpReconnect, WireProvisionedMcpServer,
};

/// Top-level `mcp.toml` shape — mirrors the daemon's `mcp_loader::McpConfig`.
/// Only the `[[server]]` tables matter to scanning; the unknown-field rule is
/// serde's default (unknown keys are ignored, matching the daemon loader).
#[derive(Debug, Deserialize)]
struct McpConfigToml {
    #[serde(default)]
    server: Vec<ServerConfigToml>,
}

/// One `[[server]]` table — a deserialize-only mirror of the daemon's
/// `mcp_loader::ServerConfig`. All fields except `name` are optional and the
/// `kind` defaults to `"stdio"`, exactly like the daemon. The wire payload the
/// scan produces is the source of truth for the daemon's later re-connection,
/// so the field values (including the `kind`/`auth.kind` strings) are carried
/// verbatim in the daemon's own vocabulary.
#[derive(Debug, Deserialize)]
struct ServerConfigToml {
    name: String,
    #[serde(default)]
    kind: ServerKindToml,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    auth: Option<AuthConfigToml>,
    #[serde(default)]
    request_timeout_ms: Option<u64>,
    #[serde(default)]
    sse_idle_timeout_ms: Option<u64>,
    #[serde(default)]
    body_cap_bytes: Option<u64>,
    #[serde(default)]
    reconnect: Option<ReconnectConfigToml>,
    #[serde(default)]
    inject_summary: bool,
    #[serde(default)]
    inject_and_run: bool,
}

/// Mirrors the daemon's `mcp_loader::ServerKind` (`snake_case`, default
/// `stdio`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ServerKindToml {
    #[default]
    Stdio,
    StreamableHttp,
}

/// Mirrors the daemon's `mcp_loader::HttpAuthConfig`.
#[derive(Debug, Deserialize)]
struct AuthConfigToml {
    kind: String,
    #[serde(default)]
    token_keychain_ref: Option<String>,
}

/// Mirrors the daemon's `mcp_loader::ReconnectConfig`.
#[derive(Debug, Deserialize)]
struct ReconnectConfigToml {
    #[serde(default)]
    initial_ms: Option<u64>,
    #[serde(default)]
    max_ms: Option<u64>,
    #[serde(default)]
    max_attempts: Option<u64>,
}

impl From<ServerConfigToml> for WireProvisionedMcpServer {
    fn from(config: ServerConfigToml) -> Self {
        WireProvisionedMcpServer {
            name: config.name,
            kind: match config.kind {
                ServerKindToml::Stdio => "stdio".to_string(),
                ServerKindToml::StreamableHttp => "streamable_http".to_string(),
            },
            command: config.command,
            args: config.args,
            endpoint: config.endpoint,
            auth: config.auth.map(Into::into),
            request_timeout_ms: config.request_timeout_ms,
            sse_idle_timeout_ms: config.sse_idle_timeout_ms,
            body_cap_bytes: config.body_cap_bytes,
            reconnect: config.reconnect.map(Into::into),
            inject_summary: config.inject_summary,
            inject_and_run: config.inject_and_run,
        }
    }
}

impl From<AuthConfigToml> for WireProvisionedMcpAuth {
    fn from(auth: AuthConfigToml) -> Self {
        WireProvisionedMcpAuth {
            kind: auth.kind,
            token_keychain_ref: auth.token_keychain_ref,
        }
    }
}

impl From<ReconnectConfigToml> for WireProvisionedMcpReconnect {
    fn from(reconnect: ReconnectConfigToml) -> Self {
        WireProvisionedMcpReconnect {
            initial_ms: reconnect.initial_ms,
            max_ms: reconnect.max_ms,
            max_attempts: reconnect.max_attempts,
        }
    }
}

/// Scan a single `config.toml` for its `[[server]]` catalog.
///
/// Unlike [`scan_mcp_servers`] this reads exactly one file — the controller's
/// `config.toml` — and yields its `[[server]]` entries verbatim (no
/// user/project merge). It is the first-chosen source: the caller uses it only
/// when it produces a non-empty list, otherwise falling back to
/// [`scan_mcp_servers`]. Diagnostics use the `(config, <path>)` label so a
/// config.toml parse failure is distinguishable from the legacy file scan.
pub(crate) fn scan_mcp_servers_from_config(
    config_path: &Path,
) -> (Vec<WireProvisionedMcpServer>, Vec<String>) {
    let mut diagnostics = Vec::new();
    let servers = read_config(config_path, "config", &mut diagnostics);
    (servers, diagnostics)
}

/// Scan the two local MCP config roots and return the merged provisioned
/// server catalog plus `(file_label, parse error)` diagnostics.
///
/// `user_toml` is read first, then `project_toml`; a project entry replaces a
/// user entry of the same name in place (order preserved), mirroring the
/// daemon's `load_all` merge loop. A missing file is the config-file-free
/// posture (no diagnostic). A read or parse failure of one file produces a
/// diagnostic while the other file's entries still come through.
pub(crate) fn scan_mcp_servers(
    user_toml: &Path,
    project_toml: &Path,
) -> (Vec<WireProvisionedMcpServer>, Vec<String>) {
    let mut servers: Vec<WireProvisionedMcpServer> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();

    for (path, label) in [(user_toml, "user"), (project_toml, "project")] {
        for config in read_config(path, label, &mut diagnostics) {
            push_replacing(&mut servers, config);
        }
    }

    (servers, diagnostics)
}

/// Read and parse one `mcp.toml` root, returning its `[[server]]` entries.
/// Diagnostics follow the daemon loader's `mcp config ({label}, {path}): …`
/// shape so the label identifies which file failed.
fn read_config(
    path: &Path,
    label: &str,
    diagnostics: &mut Vec<String>,
) -> Vec<WireProvisionedMcpServer> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            diagnostics.push(format!(
                "mcp config ({label}, {}): read failed: {e}",
                path.display()
            ));
            return Vec::new();
        }
    };
    match toml::from_str::<McpConfigToml>(&text) {
        Ok(config) => config
            .server
            .into_iter()
            .map(WireProvisionedMcpServer::from)
            .collect(),
        Err(e) => {
            diagnostics.push(format!(
                "mcp config ({label}, {}): parse failed: {e}",
                path.display()
            ));
            Vec::new()
        }
    }
}

/// Mirrors `load_all`'s position-replace: a later (project) entry with the
/// same name overwrites the earlier (user) entry in place, preserving position.
fn push_replacing(servers: &mut Vec<WireProvisionedMcpServer>, server: WireProvisionedMcpServer) {
    if let Some(i) = servers
        .iter()
        .position(|existing| existing.name == server.name)
    {
        servers[i] = server;
    } else {
        servers.push(server);
    }
}

#[cfg(test)]
// Test files live in `tests/mcp_scan/` (mirror of src), pulled in by path so
// they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("mcp_scan");
