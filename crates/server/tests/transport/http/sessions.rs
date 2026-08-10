//! `/sessions` routes (session-resource-model): list / create / switch / rename / delete
//! plus the 404 (unknown id) and 409 (running graphs) protection paths, driven over real
//! HTTP against a router wired with the in-memory [`FakeSessionOps`].

use super::super::*;
use crate::transport::testing::FakeSessionOps;
use serde_json::json;

fn web_status(session_id: &str) -> WebStatus {
    WebStatus {
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
    }
}

/// Spawn the router on a loopback port; returns base URL, the command queue the
/// session routes feed, and the server handle (abort at test end).
async fn spawn_sessions_server(
    ops: Arc<FakeSessionOps>,
    current: &str,
) -> (
    String,
    mpsc::UnboundedReceiver<WebCommand>,
    tokio::task::JoinHandle<()>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WebCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WebStatus>(16);
    let state = HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest: Arc::new(Mutex::new(web_status(current))),
        completer: SlashCompleter::from_registry(&crate::commands::Registry::with_builtins()),
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

    let response = client.get(format!("{base}/sessions")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
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

    // Body optional: create without a name.
    let response = client
        .post(format!("{base}/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    let first_id = created["session_id"].as_str().unwrap().to_string();
    assert!(first_id.starts_with("sess-new-"), "{first_id}");
    match rx.recv().await.unwrap() {
        WebCommand::SwitchSession { id } => assert_eq!(id, first_id),
        other => panic!("unexpected command: {other:?}"),
    }

    // With a name: created summary carries it.
    let response = client
        .post(format!("{base}/sessions"))
        .json(&json!({ "name": "brand new" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    assert_eq!(created["name"], "brand new");
    let second_id = created["session_id"].as_str().unwrap().to_string();
    assert_ne!(second_id, first_id);
    match rx.recv().await.unwrap() {
        WebCommand::SwitchSession { id } => assert_eq!(id, second_id),
        other => panic!("unexpected command: {other:?}"),
    }
    // Visible in the list.
    let body: serde_json::Value = client
        .get(format!("{base}/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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

    let response = client
        .post(format!("{base}/sessions/sess-b/switch"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["accepted"], true);
    match rx.recv().await.unwrap() {
        WebCommand::SwitchSession { id } => assert_eq!(id, "sess-b"),
        other => panic!("unexpected command: {other:?}"),
    }
    // /state now reports the switched session.
    let state: serde_json::Value = client
        .get(format!("{base}/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["session_id"], "sess-b");
    // And GET /sessions marks it current.
    let body: serde_json::Value = client
        .get(format!("{base}/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["current_session_id"], "sess-b");

    // Unknown id → 404.
    let response = client
        .post(format!("{base}/sessions/nope/switch"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    server.abort();
}

#[tokio::test]
async fn patch_route_renames_and_404s_unknown() {
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("sess-a");
    let (base, _rx, server) = spawn_sessions_server(ops, "sess-a").await;
    let client = reqwest::Client::new();

    let response = client
        .patch(format!("{base}/sessions/sess-a"))
        .json(&json!({ "name": "renamed" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = client
        .get(format!("{base}/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == "sess-a")
        .cloned()
        .unwrap();
    assert_eq!(session["name"], "renamed");

    // Empty name → 400; unknown id → 404.
    let response = client
        .patch(format!("{base}/sessions/sess-a"))
        .json(&json!({ "name": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let response = client
        .patch(format!("{base}/sessions/nope"))
        .json(&json!({ "name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

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

    // 409 while graphs are running (error body carries the run ids).
    let response = client
        .delete(format!("{base}/sessions/sess-busy"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("run-1"), "{body}");
    let body: serde_json::Value = client
        .get(format!("{base}/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == "sess-busy")
    );

    // Deleting the current session → 204, fallback becomes current.
    let response = client
        .delete(format!("{base}/sessions/sess-a"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    match rx.recv().await.unwrap() {
        WebCommand::SwitchSession { id } => assert_eq!(id, "sess-busy"),
        other => panic!("unexpected command: {other:?}"),
    }
    let body: serde_json::Value = client
        .get(format!("{base}/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["current_session_id"], "sess-busy");
    assert!(
        !body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == "sess-a")
    );

    // Unknown id → 404.
    let response = client
        .delete(format!("{base}/sessions/nope"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    server.abort();
}
