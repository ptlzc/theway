//! Session-scoped MCP provisioning (session-scoped-mcp): `ActivateSession`
//! carries `mcp_servers`, the daemon connects them for that session only, and
//! the session snapshot reflects the result (including per-server failures).

use super::*;

use crate::mcp_test_fixture::{bogus_stdio_server, thewayd_stdio_server};
use theway_transport::wire::WireDaemonConfig;

fn catalog_model() -> theway_llm_provider::Model {
    theway_llm_provider::list_models()
        .into_iter()
        .find(|model| SUPPORTED_APIS.contains(&model.api.0.as_str()))
        .expect("a supported model should exist in the catalog")
}

#[tokio::test]
async fn activate_session_with_mcp_servers_exposes_tools_in_snapshot() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = catalog_model();
    let mut request = activation_request(
        "client-mcp",
        &work,
        Some(&model.provider.0),
        Some(&model.id),
    );
    request.mcp_servers = vec![thewayd_stdio_server(
        &fixture._scratch,
        "session-mcp",
    )];
    let host = fixture.host();
    let (tx, rx) = oneshot::channel();

    host.handle_web_command(
        WireCommand::ActivateSession {
            request,
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(rx.await.unwrap().unwrap().created);
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.servers, 1);
    assert_eq!(snapshot.sidebar.mcp.server_names, vec!["session-mcp".to_string()]);
    assert!(
        snapshot.sidebar.mcp.tools > 0,
        "the session MCP server must contribute tools: {:?}",
        snapshot.sidebar.mcp
    );
    assert!(
        snapshot
            .sidebar
            .mcp
            .tool_names
            .iter()
            .any(|name| name == "session_list"),
        "expected the thewayd MCP tool set: {:?}",
        snapshot.sidebar.mcp.tool_names
    );
    assert!(snapshot.sidebar.mcp.errors.is_empty(), "{:?}", snapshot.sidebar.mcp.errors);
    let harness_tool_names: Vec<String> = host
        .session
        .kernel
        .harness()
        .agent()
        .state()
        .tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect();
    for name in &snapshot.sidebar.mcp.tool_names {
        assert!(
            harness_tool_names.contains(name),
            "session MCP tool {name} must be registered on the live harness"
        );
    }
}

#[tokio::test]
async fn activate_session_with_failed_mcp_server_reports_snapshot_error() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = catalog_model();
    let mut request = activation_request(
        "client-mcp-broken",
        &work,
        Some(&model.provider.0),
        Some(&model.id),
    );
    request.mcp_servers = vec![bogus_stdio_server("broken-mcp")];
    let host = fixture.host();
    let (tx, rx) = oneshot::channel();

    host.handle_web_command(
        WireCommand::ActivateSession {
            request,
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(rx.await.unwrap().unwrap().created);
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.servers, 0);
    assert!(snapshot.sidebar.mcp.tool_names.is_empty());
    assert_eq!(snapshot.sidebar.mcp.errors.len(), 1, "{:?}", snapshot.sidebar.mcp.errors);
    assert_eq!(snapshot.sidebar.mcp.errors[0].name, "broken-mcp");
    assert!(
        !snapshot.sidebar.mcp.errors[0].error.is_empty(),
        "a failed session MCP server must carry its reason"
    );
}

#[tokio::test]
async fn activate_session_mcp_server_replaces_same_name_daemon_server() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = catalog_model();
    let daemon_server = thewayd_stdio_server(&fixture._scratch, "shared");
    let session_server = thewayd_stdio_server(&fixture._scratch, "shared");
    // Daemon-level Configure provisions "shared" first.
    let mut patch = WireDaemonConfig::default();
    patch.mcp_servers = vec![daemon_server];
    let host = fixture.host();
    host.handle_configure(patch, &mut TurnState::default())
        .await;
    assert_eq!(host.wire_snapshot().sidebar.mcp.server_names, vec!["shared"]);
    // The activation request provisions a session server under the same name.
    let mut request = activation_request(
        "client-mcp-override",
        &work,
        Some(&model.provider.0),
        Some(&model.id),
    );
    request.mcp_servers = vec![session_server];
    let (tx, rx) = oneshot::channel();

    host.handle_web_command(
        WireCommand::ActivateSession {
            request,
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(rx.await.unwrap().unwrap().created);
    let snapshot = host.wire_snapshot();
    assert_eq!(
        snapshot.sidebar.mcp.server_names,
        vec!["shared".to_string()],
        "the session server must replace the daemon one, not stack with it"
    );
    let mut names = snapshot.sidebar.mcp.tool_names.clone();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(
        names.len(),
        total,
        "a same-name session server must not duplicate the daemon tool set: {:?}",
        snapshot.sidebar.mcp.tool_names
    );
}

#[tokio::test]
async fn configure_after_activation_remerges_the_session_overlay() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = catalog_model();
    let daemon_shared = thewayd_stdio_server(&fixture._scratch, "shared");
    let session_shared = thewayd_stdio_server(&fixture._scratch, "shared");
    let daemon_shared_again = thewayd_stdio_server(&fixture._scratch, "shared");
    let daemon_extra = thewayd_stdio_server(&fixture._scratch, "extra");
    let host = fixture.host();
    let mut patch = WireDaemonConfig::default();
    patch.mcp_servers = vec![daemon_shared];
    host.handle_configure(patch, &mut TurnState::default())
        .await;
    let mut request = activation_request(
        "client-mcp-remerge",
        &work,
        Some(&model.provider.0),
        Some(&model.id),
    );
    request.mcp_servers = vec![session_shared];
    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request,
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(rx.await.unwrap().unwrap().created);
    assert_eq!(
        host.wire_snapshot().sidebar.mcp.server_names,
        vec!["shared".to_string()]
    );

    // A later daemon-level Configure re-merges over the session overlay:
    // the new daemon server joins, the shadowed "shared" stays the session's.
    let mut patch = WireDaemonConfig::default();
    patch.mcp_servers = vec![daemon_shared_again, daemon_extra];
    host.handle_configure(patch, &mut TurnState::default())
        .await;

    let snapshot = host.wire_snapshot();
    let mut server_names = snapshot.sidebar.mcp.server_names.clone();
    server_names.sort();
    assert_eq!(
        server_names,
        vec!["extra".to_string(), "shared".to_string()],
        "the session overlay must survive a daemon-level Configure"
    );
    assert!(
        snapshot.sidebar.mcp.tools > 0,
        "the re-merged layer must still carry tools: {:?}",
        snapshot.sidebar.mcp
    );
    assert!(snapshot.sidebar.mcp.errors.is_empty());
}
