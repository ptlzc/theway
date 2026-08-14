//! `/sessions` routes (session-resource-model): list / create / switch / rename / delete
//! plus the 404 (unknown id) and 409 (running graphs) protection paths, driven over real
//! HTTP against a router wired with the in-memory [`FakeSessionOps`].

use super::super::*;
use crate::wire::WireContextUsage;
use super::helpers::{rpc_call, rpc_error};
use crate::testing::{FakeSessionOps, empty_sidebar_snapshot};
use serde_json::json;

fn wire_status(session_id: &str) -> WireStatus {
    WireStatus {
        session_id: session_id.into(),
        model: "provider:model".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_lines: Vec::new(),
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
    }
}

/// Spawn the router on a loopback port; returns base URL, the command queue the
/// session routes feed, and the server handle (abort at test end).
async fn spawn_sessions_server(
    ops: Arc<FakeSessionOps>,
    current: &str,
) -> (
    String,
    mpsc::UnboundedReceiver<WireCommand>,
    tokio::task::JoinHandle<()>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
    let state = HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest: Arc::new(Mutex::new(wire_status(current))),
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: broadcast::channel::<AgentJobEvent>(16).0,
        dag_events: broadcast::channel::<DagEvent>(16).0,
        registry: AgentJobRegistry::new(),
        session_ops: ops,
    };
    let router = web_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    (format!("http://{addr}"), command_rx, server)
}

#[tokio::test]
async fn get_sessions_lists_all_and_marks_current() {
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("sess-a");
    ops.add_session("sess-b");
    let (base, _rx, server) = spawn_sessions_server(ops, "sess-a").await;
    let client = reqwest::Client::new();

    let body = rpc_call(&client, &base, 1, "list_sessions", None).await;
    assert_eq!(body["current_session_id"], "sess-a");
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().any(|s| s["session_id"] == "sess-a"));
    assert!(sessions.iter().any(|s| s["session_id"] == "sess-b"));

    server.abort();
}

#[tokio::test]
async fn post_sessions_creates_renames_and_switches() {
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("sess-a");
    let (base, mut rx, server) = spawn_sessions_server(ops.clone(), "sess-a").await;
    let client = reqwest::Client::new();

    // Params optional: create without a name.
    let created = rpc_call(&client, &base, 1, "create_session", None).await;
    let first_id = created["session_id"].as_str().unwrap().to_string();
    assert!(first_id.starts_with("sess-new-"), "{first_id}");
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, first_id),
        other => panic!("unexpected command: {other:?}"),
    }

    // With a name: created summary carries it.
    let created = rpc_call(
        &client,
        &base,
        2,
        "create_session",
        Some(json!({ "name": "brand new" })),
    )
    .await;
    assert_eq!(created["name"], "brand new");
    let second_id = created["session_id"].as_str().unwrap().to_string();
    assert_ne!(second_id, first_id);
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, second_id),
        other => panic!("unexpected command: {other:?}"),
    }
    // Visible in the list.
    let body = rpc_call(&client, &base, 3, "list_sessions", None).await;
    assert!(
        body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == second_id && s["name"] == "brand new")
    );

    server.abort();
}

#[tokio::test]
async fn switch_route_rebinds_current_and_404s_unknown() {
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("sess-a");
    ops.add_session("sess-b");
    let (base, mut rx, server) = spawn_sessions_server(ops, "sess-a").await;
    let client = reqwest::Client::new();

    let body = rpc_call(
        &client,
        &base,
        4,
        "switch_session",
        Some(json!({ "id": "sess-b" })),
    )
    .await;
    assert_eq!(body["accepted"], true);
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, "sess-b"),
        other => panic!("unexpected command: {other:?}"),
    }
    // /state now reports the switched session.
    let state = rpc_call(&client, &base, 5, "get_state", None).await;
    assert_eq!(state["session_id"], "sess-b");
    // And GET /sessions marks it current.
    let body = rpc_call(&client, &base, 6, "list_sessions", None).await;
    assert_eq!(body["current_session_id"], "sess-b");

    // Unknown id → -32004.
    let (code, _msg) = rpc_error(
        &client,
        &base,
        7,
        "switch_session",
        Some(json!({ "id": "nope" })),
    )
    .await;
    assert_eq!(code, -32004);

    server.abort();
}

#[tokio::test]
async fn patch_route_renames_and_404s_unknown() {
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("sess-a");
    let (base, _rx, server) = spawn_sessions_server(ops, "sess-a").await;
    let client = reqwest::Client::new();

    let renamed = rpc_call(
        &client,
        &base,
        8,
        "rename_session",
        Some(json!({ "id": "sess-a", "name": "renamed" })),
    )
    .await;
    assert_eq!(renamed["accepted"], true);
    let body = rpc_call(&client, &base, 9, "list_sessions", None).await;
    let session = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == "sess-a")
        .cloned()
        .unwrap();
    assert_eq!(session["name"], "renamed");

    // Empty name → -32602; unknown id → -32004.
    let (code, _m) = rpc_error(
        &client,
        &base,
        10,
        "rename_session",
        Some(json!({ "id": "sess-a", "name": "   " })),
    )
    .await;
    assert_eq!(code, -32602);
    let (code, _m) = rpc_error(
        &client,
        &base,
        11,
        "rename_session",
        Some(json!({ "id": "nope", "name": "x" })),
    )
    .await;
    assert_eq!(code, -32004);

    server.abort();
}

#[tokio::test]
async fn delete_route_removes_conflicts_on_active_and_404s_unknown() {
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("sess-a");
    ops.add_session("sess-busy");
    ops.set_running("sess-busy", &["run-1"]);
    let (base, mut rx, server) = spawn_sessions_server(ops, "sess-a").await;
    let client = reqwest::Client::new();

    // -32009 while graphs are running (error message carries the run ids).
    let (code, msg) = rpc_error(
        &client,
        &base,
        12,
        "delete_session",
        Some(json!({ "id": "sess-busy" })),
    )
    .await;
    assert_eq!(code, -32009);
    assert!(msg.contains("run-1"), "{msg}");
    let body = rpc_call(&client, &base, 13, "list_sessions", None).await;
    assert!(
        body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == "sess-busy")
    );

    // Deleting the current session → fallback becomes current.
    let deleted = rpc_call(
        &client,
        &base,
        14,
        "delete_session",
        Some(json!({ "id": "sess-a" })),
    )
    .await;
    assert_eq!(deleted["deleted"], true);
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, "sess-busy"),
        other => panic!("unexpected command: {other:?}"),
    }
    let body = rpc_call(&client, &base, 15, "list_sessions", None).await;
    assert_eq!(body["current_session_id"], "sess-busy");
    assert!(
        !body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == "sess-a")
    );

    // Unknown id → -32004.
    let (code, _m) = rpc_error(
        &client,
        &base,
        16,
        "delete_session",
        Some(json!({ "id": "nope" })),
    )
    .await;
    assert_eq!(code, -32004);

    server.abort();
}
