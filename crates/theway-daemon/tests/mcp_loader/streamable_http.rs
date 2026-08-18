//! Tests for `mcp_loader` streamable-HTTP connection assembly and auth
//! resolution — split out of `mod.rs` (see docs/rust-test-files.md).

use super::*;

use crate::test_env::{ENV_LOCK, EnvGuard};
use theway_mcp::HttpMcpAuth;
use theway_transport::auth::{AuthStore, ProviderCredential};

fn streamable_http_server(endpoint: &str) -> ServerConfig {
    let mut server = http_server(endpoint);
    server.name = "streamable-http-server".into();
    server
}

#[tokio::test]
async fn connect_streamable_http_success_applies_all_options() {
    // Arrange
    let mut server = streamable_http_server("http://127.0.0.1:9/mcp");
    server.request_timeout_ms = Some(1_234);
    server.sse_idle_timeout_ms = Some(5_678);
    server.body_cap_bytes = Some(999);
    server.reconnect = Some(ReconnectConfig {
        initial_ms: Some(100),
        max_ms: Some(2_000),
        max_attempts: Some(3),
    });

    // Act
    let client = connect_streamable_http(&server)
        .await
        .expect("connect_streamable_http should assemble a client without network I/O");

    // Assert: assembly succeeded and cleanup does not hang.
    client.close().await;
}

#[tokio::test]
async fn connect_streamable_http_success_applies_reconnect_defaults() {
    // Arrange
    let mut server = streamable_http_server("http://127.0.0.1:9/mcp");
    server.reconnect = Some(ReconnectConfig {
        initial_ms: None,
        max_ms: None,
        max_attempts: Some(2),
    });

    // Act
    let client = connect_streamable_http(&server)
        .await
        .expect("connect_streamable_http should assemble a client without network I/O");

    // Assert: assembly succeeded and cleanup does not hang.
    client.close().await;
}

#[tokio::test]
async fn connect_one_streamable_http_missing_endpoint_returns_err() {
    // Arrange
    let mut server = streamable_http_server("");
    server.endpoint = None;

    // Act
    let err = match connect_one(&server).await {
        Ok(_) => panic!("streamable_http without endpoint should fail"),
        Err(err) => err,
    };

    // Assert
    assert!(err.to_string().contains("missing endpoint"), "{err}");
}

#[test]
fn resolve_http_auth_none_returns_none() {
    // Arrange & Act
    let auth = resolve_http_auth(None).unwrap();

    // Assert
    assert!(matches!(auth, HttpMcpAuth::None));
}

#[test]
fn resolve_http_auth_loads_bearer_from_auth_store() {
    // Arrange
    let _env_lock = ENV_LOCK.lock().unwrap();
    let theway_dir = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", theway_dir.path());
    let mut store = AuthStore::default();
    store.set(
        "mcp-example:default",
        ProviderCredential::ApiKey {
            value: "secret-token".into(),
        },
    );
    store.save_to(&theway_dir.path().join("auth.json")).unwrap();

    // Act
    let auth = resolve_http_auth(Some(&HttpAuthConfig {
        kind: "bearer".into(),
        token_keychain_ref: Some("mcp-example:default".into()),
    }))
    .unwrap();

    // Assert
    match auth {
        HttpMcpAuth::Bearer { token } => assert_eq!(token, "secret-token"),
        other => panic!("expected bearer auth, got {other:?}"),
    }
}

#[test]
fn resolve_http_auth_reports_store_load_failure_with_recovery_hint() {
    // Arrange
    let _env_lock = ENV_LOCK.lock().unwrap();
    let theway_dir = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", theway_dir.path());
    std::fs::write(theway_dir.path().join("auth.json"), "{ not valid json").unwrap();

    // Act
    let err = resolve_http_auth(Some(&HttpAuthConfig {
        kind: "bearer".into(),
        token_keychain_ref: Some("mcp-example:default".into()),
    }))
    .unwrap_err()
    .to_string();

    // Assert
    assert!(
        err.contains("failed to load local credential store"),
        "{err}"
    );
    assert!(err.contains("run /login <configured-token-ref>"), "{err}");
}
