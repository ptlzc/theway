//! Tests for `dag_persist` — split out of src (see docs/rust-test-files.md).

use super::*;
use theway_core::multiagent::graph::engine::NodeLauncher;
use theway_core::multiagent::graph::types::{DagNodeDef, DagRunDef};

struct NoopLauncher;

impl NodeLauncher for NoopLauncher {
    fn launch(&self, _run_id: &str, _node_id: &str, _cancel: tokio_util::sync::CancellationToken) {}
}

fn node_def(id: &str) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "main-agent".to_string(),
        task: "do the thing".to_string(),
        depends_on: None,
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(name: &str) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: vec![node_def("n1")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

fn handle_without_task(engine: Arc<DagEngine>, cwd: std::path::PathBuf) -> DagPersistHandle {
    DagPersistHandle {
        engine,
        cwd,
        stores: Mutex::new(HashMap::new()),
        dirty: Arc::new(Notify::new()),
        task: Mutex::new(None),
    }
}

#[tokio::test]
async fn store_for_reuses_stores_by_session_id() {
    // Arrange
    let cwd = tempfile::tempdir().unwrap().path().to_path_buf();
    let handle = handle_without_task(Arc::new(DagEngine::new()), cwd);

    // Act
    handle.store_for(Some("sess-1")).await.unwrap();
    handle.store_for(Some("sess-1")).await.unwrap();
    handle.store_for(None).await.unwrap();

    // Assert: repeated session id reuses the open store; None gets its own.
    assert_eq!(handle.stores.lock().len(), 2);
}

#[tokio::test]
async fn notify_dirty_signals_the_coalescing_loop() {
    // Arrange
    let cwd = tempfile::tempdir().unwrap().path().to_path_buf();
    let handle = handle_without_task(Arc::new(DagEngine::new()), cwd);

    // Act
    handle.notify_dirty();

    // Assert: the loop's `Notified` future completes without a timeout.
    let notified = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        handle.dirty.notified(),
    )
    .await;
    assert!(notified.is_ok(), "notify_dirty must wake the debounce loop");
}

#[tokio::test]
async fn run_loop_debounces_dirty_notifications_and_saves() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    engine
        .plan(run_def("debounced-run"), None, Some("sess-1".into()))
        .unwrap();
    let handle = DagPersistHandle::spawn(engine.clone(), cwd.clone());

    // Act: wake the background loop; it should coalesce and save within the
    // 500 ms debounce window.
    handle.notify_dirty();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut runs = load_session_runs(&cwd, "sess-1").await;
    while runs.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        runs = load_session_runs(&cwd, "sess-1").await;
    }

    // Assert
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].name, "debounced-run");

    if let Some(task) = handle.task.lock().take() {
        task.abort();
    }
}

#[tokio::test]
async fn spawn_wires_sink_and_flush_persists_running_runs_grouped_by_session() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    engine
        .plan(run_def("first-in-sess-1"), None, Some("sess-1".into()))
        .unwrap();
    engine
        .plan(run_def("second-in-sess-1"), None, Some("sess-1".into()))
        .unwrap();
    engine
        .plan(run_def("only-in-sess-2"), None, Some("sess-2".into()))
        .unwrap();

    // Act
    let handle = DagPersistHandle::spawn(engine.clone(), cwd.clone());
    assert!(handle.task.lock().is_some());
    handle.flush().await;

    // Assert: each run landed in its owning session's state file.
    let sess1 = load_session_runs(&cwd, "sess-1").await;
    let sess2 = load_session_runs(&cwd, "sess-2").await;
    assert_eq!(sess1.len(), 2);
    assert_eq!(sess2.len(), 1);
    assert!(sess1
        .iter()
        .all(|r| r.session_id.as_deref() == Some("sess-1")));
    assert_eq!(sess2[0].session_id.as_deref(), Some("sess-2"));
    assert!(sess1.iter().any(|r| r.name == "first-in-sess-1"));
    assert!(sess1.iter().any(|r| r.name == "second-in-sess-1"));
    assert_eq!(sess2[0].name, "only-in-sess-2");

    // Cleanup: stop the background debounce task so it can't outlive the tempdir.
    if let Some(task) = handle.task.lock().take() {
        task.abort();
    }
}

#[tokio::test]
async fn load_session_runs_returns_empty_for_missing_state() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();

    // Act
    let runs = load_session_runs(dir.path(), "never-written").await;

    // Assert
    assert!(runs.is_empty());
}
