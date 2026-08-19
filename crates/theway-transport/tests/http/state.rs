//! Runtime-state storage JSON-RPC methods (issue #84): e2e over `POST /rpc`
//! against a router wired with the in-memory [`FakeStorageOps`] — the JSON
//! twin of the gRPC `StorageService` surface.

use super::super::*;
use super::helpers::{rpc_call, rpc_error};
use crate::testing::{FakeSessionOps, FakeStorageOps, empty_sidebar_snapshot};
use crate::wire::WireContextUsage;
use serde_json::json;

/// Spawn the router with a seeded `FakeStorageOps`; returns the base URL, the
/// fake (for seeding/inspection) and the server handle (abort at test end).
async fn spawn_state_server() -> (
    String,
    std::sync::Arc<FakeStorageOps>,
    tokio::task::JoinHandle<()>,
) {
    let storage = std::sync::Arc::new(FakeStorageOps::new());
    let (command_tx, _command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let state = HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest: Arc::new(Mutex::new(WireStatus {
            session_id: "sess-1".into(),
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
            feed_blocks_base: 0,
            feed_block_patches: Vec::new(),
            feed_lines: Vec::new(),
            feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: WireContextUsage::default(),
            tui_max_feed_lines: None,
        })),
        completer: SlashCompleter::from_commands(vec!["/help".into()]),
        events: broadcast::channel::<WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<WireDagEvent>(16).0,
        job_ops: Arc::new(crate::UnavailableJobOps),
        session_ops: Arc::new(FakeSessionOps::new()),
        path_context: std::sync::Arc::new(std::sync::RwLock::new(
            crate::wire::WirePathContext::default(),
        )),
        daemon_config: std::sync::Arc::new(std::sync::RwLock::new(
            crate::wire::WireDaemonConfig::default(),
        )),
        tool_ops: std::sync::Arc::new(crate::testing::FakeToolOps::new()),
        storage_ops: storage.clone(),
    };
    let router = web_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    (format!("http://{addr}"), storage, server)
}

#[tokio::test]
async fn json_rpc_state_dag_trigger_cron_round_trip() {
    let (base, _storage, server) = spawn_state_server().await;
    let client = reqwest::Client::new();

    // DAG run save/load.
    let result = rpc_call(
        &client,
        &base,
        1,
        "state.save_dag_run",
        Some(json!({
            "session_id": "sess-1",
            "run_id": "dag-1",
            "snapshot": r#"{"id":"dag-1","name":"build"}"#
        })),
    )
    .await;
    assert_eq!(result["saved"], true);
    let result = rpc_call(
        &client,
        &base,
        2,
        "state.load_dag_runs",
        Some(json!({ "session_id": "sess-1" })),
    )
    .await;
    assert_eq!(result["runs"].as_array().unwrap().len(), 1);
    assert_eq!(result["runs"][0]["run_id"], "dag-1");
    assert_eq!(result["runs"][0]["snapshot"], r#"{"id":"dag-1","name":"build"}"#);

    // Trigger rules save/load.
    let result = rpc_call(
        &client,
        &base,
        3,
        "state.save_trigger_rules",
        Some(json!({
            "session_id": "sess-1",
            "rules": [{
                "id": "tr-1",
                "condition": "file changes",
                "action": "run test",
                "enabled": true,
                "fire_once": false,
                "promote_to_chat": true,
                "created_at": "2026-01-01T00:00:00Z"
            }]
        })),
    )
    .await;
    assert_eq!(result["count"], 1);
    let result = rpc_call(
        &client,
        &base,
        4,
        "state.load_trigger_rules",
        Some(json!({ "session_id": "sess-1" })),
    )
    .await;
    assert_eq!(result["rules"][0]["id"], "tr-1");
    assert_eq!(result["rules"][0]["action"], "run test");

    // Cron jobs save/load.
    let result = rpc_call(
        &client,
        &base,
        5,
        "state.save_cron_jobs",
        Some(json!({
            "session_id": "sess-1",
            "jobs": [{
                "id": "cron-1",
                "schedule": "*/5 * * * *",
                "action": "backup",
                "enabled": true,
                "stateful": false,
                "created_at": "2026-01-01T00:00:00Z"
            }]
        })),
    )
    .await;
    assert_eq!(result["count"], 1);
    let result = rpc_call(
        &client,
        &base,
        6,
        "state.load_cron_jobs",
        Some(json!({ "session_id": "sess-1" })),
    )
    .await;
    assert_eq!(result["jobs"][0]["id"], "cron-1");
    assert_eq!(result["jobs"][0]["schedule"], "*/5 * * * *");

    // The `storage.` namespace alias reaches the same handler.
    let result = rpc_call(
        &client,
        &base,
        7,
        "storage.load_cron_jobs",
        Some(json!({ "session_id": "sess-1" })),
    )
    .await;
    assert_eq!(result["jobs"][0]["id"], "cron-1");

    // Unknown method remains unknown.
    let (code, _msg) = rpc_error(
        &client,
        &base,
        8,
        "state.load_sessions",
        None,
    )
    .await;
    assert_eq!(code, -32601);

    server.abort();
}
