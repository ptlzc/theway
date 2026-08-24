//! Tests for `mcp_loader` — split out of src (see docs/rust-test-files.md).

use super::*;

use std::path::Path;
use std::sync::{Arc, RwLock};

use theway_transport::auth::AuthStore;

mod load_all;
mod streamable_http;

fn test_paths() -> (tempfile::TempDir, tempfile::TempDir, crate::DaemonPaths) {
    let work = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: work.path().to_path_buf(),
        extra_skill_dirs: Arc::new(RwLock::new(Vec::new())),
    };
    (work, base, paths)
}

fn stdio_server(name: &str) -> ServerConfig {
    ServerConfig {
        name: name.into(),
        kind: ServerKind::Stdio,
        command: Some(format!("/definitely/not/a/real/path/for/mcp/{name}")),
        args: vec![],
        endpoint: None,
        auth: None,
        request_timeout_ms: None,
        sse_idle_timeout_ms: None,
        body_cap_bytes: None,
        reconnect: None,
        inject_summary: false,
        inject_and_run: false,
    }
}

async fn stdio_err(server: &ServerConfig, cwd: &Path) -> String {
    match connect_stdio(server, cwd).await {
        Ok(_) => panic!("stdio server should fail"),
        Err(err) => err.to_string(),
    }
}

async fn streamable_http_err(server: &ServerConfig, auth_path: &Path) -> String {
    match connect_streamable_http(server, auth_path).await {
        Ok(_) => panic!("streamable_http server should fail"),
        Err(err) => err.to_string(),
    }
}

fn http_server(endpoint: &str) -> ServerConfig {
    ServerConfig {
        name: "http-server".into(),
        kind: ServerKind::StreamableHttp,
        command: None,
        args: vec![],
        endpoint: Some(endpoint.into()),
        auth: None,
        request_timeout_ms: None,
        sse_idle_timeout_ms: None,
        body_cap_bytes: None,
        reconnect: None,
        inject_summary: false,
        inject_and_run: false,
    }
}

/// Two configured servers both fail to start (executable does not exist). Verify
/// `client_count` reports 0 (not 2), and each failure surfaces a diagnostic. Pinned
/// behavior for code-review item #9: the TUI startup banner reads from this field.
#[tokio::test]
async fn client_count_reflects_successful_connections_not_attempts() {
    let configs = vec![stdio_server("broken-a"), stdio_server("broken-b")];
    let (_work, _base, paths) = test_paths();
    let (tools, hooks, diagnostics, client_count, server_names) =
        connect_all(&configs, &paths.work_dir, &paths.base.join("auth.json")).await;
    assert_eq!(client_count, 0, "no server should be reported as connected");
    assert!(server_names.is_empty());
    assert!(tools.is_empty(), "no tools should load from failed servers");
    assert!(
        hooks.is_empty(),
        "no notification hooks should be created for failed servers"
    );
    assert_eq!(
        diagnostics.len(),
        2,
        "each failed server should emit a diagnostic, got: {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|d| d.contains("broken-a")),
        "diagnostic should mention server name 'broken-a': {diagnostics:?}"
    );
    assert!(
        diagnostics.iter().any(|d| d.contains("broken-b")),
        "diagnostic should mention server name 'broken-b': {diagnostics:?}"
    );
}

/// Empty config list ⇒ zero attempts, zero connections, zero diagnostics. Sanity check
/// the helper doesn't crash on the empty path.
#[tokio::test]
async fn empty_configs_reports_zero() {
    let (_work, _base, paths) = test_paths();
    let (tools, hooks, diagnostics, client_count, server_names) =
        connect_all(&[], &paths.work_dir, &paths.base.join("auth.json")).await;
    assert!(tools.is_empty());
    assert!(hooks.is_empty());
    assert!(diagnostics.is_empty());
    assert_eq!(client_count, 0);
    assert!(server_names.is_empty());
}

#[test]
fn streamable_http_config_deserializes_with_bearer_ref() {
    let cfg: McpConfig = toml::from_str(
        r#"
[[server]]
name = "remote-docs"
kind = "streamable_http"
endpoint = "https://mcp.example.com/mcp"
auth = { kind = "bearer", token_keychain_ref = "mcp-example:default" }
request_timeout_ms = 30000
sse_idle_timeout_ms = 60000
body_cap_bytes = 1048576
"#,
    )
    .unwrap();
    assert_eq!(cfg.server.len(), 1);
    let server = &cfg.server[0];
    assert_eq!(server.name, "remote-docs");
    assert_eq!(server.kind, ServerKind::StreamableHttp);
    assert_eq!(
        server.endpoint.as_deref(),
        Some("https://mcp.example.com/mcp")
    );
    assert_eq!(
        server
            .auth
            .as_ref()
            .and_then(|auth| auth.token_keychain_ref.as_deref()),
        Some("mcp-example:default")
    );
}

#[tokio::test]
async fn streamable_http_rejects_command_args() {
    let mut server = http_server("https://mcp.example.com/mcp");
    server.command = Some("node".into());
    server.args = vec!["server.js".into()];
    let (_work, _base, paths) = test_paths();
    let err = match connect_streamable_http(&server, &paths.base.join("auth.json")).await {
        Ok(_) => panic!("streamable_http with command/args should fail"),
        Err(err) => err,
    };
    assert!(
        err.to_string()
            .contains("must set endpoint, not command/args"),
        "{err}"
    );
}

#[test]
fn streamable_http_auth_resolves_from_auth_store_without_debug_leak() {
    let token = "mcp_token_should_not_leak";
    let mut store = AuthStore::default();
    store.set(
        "remote-docs:default",
        theway_transport::auth::ProviderCredential::ApiKey {
            value: token.into(),
        },
    );

    let auth = resolve_http_auth_from_store(
        Some(&HttpAuthConfig {
            kind: "bearer".into(),
            token_keychain_ref: Some("remote-docs:default".into()),
        }),
        &store,
    )
    .unwrap();
    let debug = format!("{auth:?}");
    assert!(!debug.contains(token), "{debug}");
    assert!(debug.contains("<redacted>"), "{debug}");
}

#[test]
fn streamable_http_missing_auth_diagnostic_does_not_echo_token_ref() {
    let store = AuthStore::default();

    let secret_like_ref = "secret_ref_should_not_leak";
    let err = resolve_http_auth_from_store(
        Some(&HttpAuthConfig {
            kind: "bearer".into(),
            token_keychain_ref: Some(secret_like_ref.into()),
        }),
        &store,
    )
    .unwrap_err()
    .to_string();
    assert!(!err.contains(secret_like_ref), "{err}");
    assert!(err.contains("<configured-token-ref>"), "{err}");
}

#[tokio::test]
async fn read_config_missing_file_returns_none_without_diagnostics() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("missing").join("mcp.toml");
    let mut diagnostics = Vec::new();

    let cfg = read_config(&path, &mut diagnostics, "project").await;

    assert!(cfg.is_none());
    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn read_config_parse_error_reports_label_and_path() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mcp.toml");
    std::fs::write(&path, "this is not [[valid toml").unwrap();
    let mut diagnostics = Vec::new();

    let cfg = read_config(&path, &mut diagnostics, "project").await;

    assert!(cfg.is_none());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("project"), "{diagnostics:?}");
    assert!(diagnostics[0].contains("parse failed"), "{diagnostics:?}");
    assert!(
        diagnostics[0].contains(&path.display().to_string()),
        "{diagnostics:?}"
    );
}

#[tokio::test]
async fn read_config_valid_file_returns_servers_with_default_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mcp.toml");
    std::fs::write(
        &path,
        r#"
[[server]]
name = "local"
command = "node"
args = ["server.js"]
"#,
    )
    .unwrap();
    let mut diagnostics = Vec::new();

    let cfg = read_config(&path, &mut diagnostics, "user").await.unwrap();

    assert!(diagnostics.is_empty());
    assert_eq!(cfg.server.len(), 1);
    assert_eq!(cfg.server[0].name, "local");
    assert_eq!(cfg.server[0].kind, ServerKind::Stdio);
    assert_eq!(cfg.server[0].command.as_deref(), Some("node"));
    assert_eq!(cfg.server[0].args, vec!["server.js".to_string()]);
}

#[tokio::test]
async fn connect_stdio_rejects_endpoint_or_auth() {
    let mut server = stdio_server("hybrid");
    server.endpoint = Some("https://mcp.example.com/mcp".into());
    server.auth = Some(HttpAuthConfig {
        kind: "bearer".into(),
        token_keychain_ref: Some("some-ref".into()),
    });

    let (_work, _base, paths) = test_paths();
    let err = stdio_err(&server, &paths.work_dir).await;

    assert!(err.contains("must not set endpoint or auth"), "{err}");
}

#[tokio::test]
async fn connect_stdio_requires_command() {
    let mut server = stdio_server("no-command");
    server.command = None;

    let (_work, _base, paths) = test_paths();
    let err = stdio_err(&server, &paths.work_dir).await;

    assert!(err.contains("missing command"), "{err}");
}

#[tokio::test]
async fn connect_streamable_http_requires_endpoint() {
    let mut server = http_server("");
    server.endpoint = None;

    let (_work, _base, paths) = test_paths();
    let err = streamable_http_err(&server, &paths.base.join("auth.json")).await;

    assert!(err.contains("missing endpoint"), "{err}");
}

#[tokio::test]
async fn connect_streamable_http_rejects_zero_request_timeout() {
    let mut server = http_server("https://mcp.example.com/mcp");
    server.request_timeout_ms = Some(0);

    let (_work, _base, paths) = test_paths();
    let err = streamable_http_err(&server, &paths.base.join("auth.json")).await;

    assert!(err.contains("request_timeout_ms must be positive"), "{err}");
}

#[tokio::test]
async fn connect_streamable_http_rejects_zero_sse_idle_timeout() {
    let mut server = http_server("https://mcp.example.com/mcp");
    server.sse_idle_timeout_ms = Some(0);

    let (_work, _base, paths) = test_paths();
    let err = streamable_http_err(&server, &paths.base.join("auth.json")).await;

    assert!(err.contains("sse_idle_timeout_ms must be positive"), "{err}");
}

#[tokio::test]
async fn connect_streamable_http_rejects_zero_body_cap() {
    let mut server = http_server("https://mcp.example.com/mcp");
    server.body_cap_bytes = Some(0);

    let (_work, _base, paths) = test_paths();
    let err = streamable_http_err(&server, &paths.base.join("auth.json")).await;

    assert!(err.contains("body_cap_bytes must be positive"), "{err}");
}

#[tokio::test]
async fn connect_streamable_http_rejects_zero_reconnect_delays() {
    let mut server = http_server("https://mcp.example.com/mcp");
    server.reconnect = Some(ReconnectConfig {
        initial_ms: Some(0),
        max_ms: Some(30_000),
        max_attempts: None,
    });

    let (_work, _base, paths) = test_paths();
    let err = streamable_http_err(&server, &paths.base.join("auth.json")).await;
    assert!(err.contains("reconnect delays must be positive"), "{err}");

    server.reconnect = Some(ReconnectConfig {
        initial_ms: Some(500),
        max_ms: Some(0),
        max_attempts: None,
    });

    let (_work, _base, paths) = test_paths();
    let err = streamable_http_err(&server, &paths.base.join("auth.json")).await;
    assert!(err.contains("reconnect delays must be positive"), "{err}");
}

#[test]
fn resolve_http_auth_from_store_none_returns_none() {
    let auth = resolve_http_auth_from_store(None, &AuthStore::default()).unwrap();
    assert!(matches!(auth, HttpMcpAuth::None));
}

#[test]
fn resolve_http_auth_from_store_rejects_unsupported_kind() {
    let err = resolve_http_auth_from_store(
        Some(&HttpAuthConfig {
            kind: "api_key".into(),
            token_keychain_ref: Some("some-ref".into()),
        }),
        &AuthStore::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("expected bearer"), "{err}");
}

#[test]
fn resolve_http_auth_from_store_requires_token_keychain_ref() {
    let err = resolve_http_auth_from_store(
        Some(&HttpAuthConfig {
            kind: "bearer".into(),
            token_keychain_ref: None,
        }),
        &AuthStore::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("requires token_keychain_ref"), "{err}");
}

#[test]
fn http_auth_recovery_pins_login_hint() {
    assert_eq!(
        http_auth_recovery(&HttpAuthConfig {
            kind: "bearer".into(),
            token_keychain_ref: Some("secret-ref".into()),
        }),
        "run /login <configured-token-ref>"
    );
}

#[cfg(unix)]
fn write_fake_mcp_server(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join("fake-mcp.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -n 's/^.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake-mcp","version":"1.0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -n 's/^.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"fake_tool","description":"a fake tool","inputSchema":{"type":"object"}}]}}\n' "$id"
      exit 0
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    script
}

#[cfg(unix)]
#[tokio::test]
async fn connect_all_returns_tools_and_hook_for_fake_stdio_server() {
    let tmp = tempfile::tempdir().unwrap();
    let script = write_fake_mcp_server(tmp.path());
    let mut fake = stdio_server("fake-success");
    fake.command = Some(script.to_string_lossy().to_string());
    let configs = vec![fake, stdio_server("broken-after-fake")];
    let (_work, _base, paths) = test_paths();

    let (tools, hooks, diagnostics, client_count, server_names) =
        connect_all(&configs, &paths.work_dir, &paths.base.join("auth.json")).await;

    assert_eq!(client_count, 1);
    assert_eq!(server_names, vec!["fake-success".to_string()]);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].definition().name, "fake_tool");
    assert_eq!(hooks.len(), 1);
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].contains("broken-after-fake"),
        "{diagnostics:?}"
    );
}
