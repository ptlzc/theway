//! Tests for `mcp_loader` streamable-HTTP connection assembly and auth
//! resolution — split out of `mod.rs` (see docs/rust-test-files.md).

use super::*;

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
    let (_work, _base, paths) = test_paths();
    let client = connect_streamable_http(&server, &paths.base.join("auth.json"))
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
    let (_work, _base, paths) = test_paths();
    let client = connect_streamable_http(&server, &paths.base.join("auth.json"))
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
    let (_work, _base, paths) = test_paths();
    let err = match connect_one(&server, &paths.work_dir, &paths.base.join("auth.json")).await {
        Ok(_) => panic!("streamable_http without endpoint should fail"),
        Err(err) => err,
    };

    // Assert
    assert!(err.to_string().contains("missing endpoint"), "{err}");
}

#[test]
fn resolve_http_auth_none_returns_none() {
    // Arrange & Act
    let (_work, _base, paths) = test_paths();
    let auth = resolve_http_auth(None, &paths.base.join("auth.json")).unwrap();

    // Assert
    assert!(matches!(auth, HttpMcpAuth::None));
}

#[test]
fn resolve_http_auth_loads_bearer_from_auth_store() {
    // Arrange
    let (_work, base, _paths) = test_paths();
    let auth_path = base.path().join("auth.json");
    let mut store = AuthStore::default();
    store.set(
        "mcp-example:default",
        ProviderCredential::ApiKey {
            value: "secret-token".into(),
        },
    );
    store.save_to(&auth_path).unwrap();

    // Act
    let auth = resolve_http_auth(
        Some(&HttpAuthConfig {
            kind: "bearer".into(),
            token_keychain_ref: Some("mcp-example:default".into()),
        }),
        &auth_path,
    )
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
    let (_work, base, _paths) = test_paths();
    let auth_path = base.path().join("auth.json");
    std::fs::write(&auth_path, "{ not valid json").unwrap();

    // Act
    let err = resolve_http_auth(
        Some(&HttpAuthConfig {
            kind: "bearer".into(),
            token_keychain_ref: Some("mcp-example:default".into()),
        }),
        &auth_path,
    )
    .unwrap_err()
    .to_string();

    // Assert
    assert!(
        err.contains("failed to load local credential store"),
        "{err}"
    );
    assert!(err.contains("run /login <configured-token-ref>"), "{err}");
}
