//! Tests for `dag_persist` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::sync::Arc;
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
        sessions: SessionExecutionRegistry::new(),
        stores: Mutex::new(HashMap::new()),
        dirty: Arc::new(Notify::new()),
        task: Mutex::new(None),
    }
}

async fn test_context(
    cwd: &std::path::Path,
    session_id: &str,
    base: &std::path::Path,
) -> Arc<crate::orchestration::SessionExecutionContext> {
    let paths = crate::DaemonPaths {
        base: base.to_path_buf(),
        home: base.to_path_buf(),
        work_dir: cwd.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let storage = crate::runtime_storage::local_runtime_storage();
    let repo = Arc::new(theway_storage::sqlite_repo::SqliteSessionRepo::new(
        base.join("repo"),
    ));
    let resources = crate::orchestration::session::SessionProjectResources::load(
        &paths,
        &[],
        &[],
        false,
    )
    .await
    .unwrap();
    let hooks = crate::orchestration::SessionHookResources::load(&paths, false).await;
    Arc::new(crate::orchestration::SessionExecutionContext::new(
        session_id.to_string(),
        cwd.to_path_buf(),
        repo,
        storage,
        paths,
        crate::executor::executor_for_cwd(cwd.to_path_buf()),
        theway_llm_provider::get_model(
            &theway_llm_provider::Provider::from("openai"),
            "gpt-4o-mini",
        )
        .expect("openai catalog model"),
        theway_core::ThinkingLevel::Off,
        resources,
        crate::orchestration::SessionMcpResources::default(),
        hooks,
    ))
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
async fn store_for_routes_to_registered_session_cwd_and_falls_back() {
    let startup = tempfile::tempdir().unwrap();
    let session_a = tempfile::tempdir().unwrap();
    let session_b = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let sessions = SessionExecutionRegistry::new();
    sessions.set_context(
        "sess-a".to_string(),
        test_context(session_a.path(), "sess-a", base.path()).await,
    );
    sessions.set_context(
        "sess-b".to_string(),
        test_context(session_b.path(), "sess-b", base.path()).await,
    );
    let handle = DagPersistHandle {
        engine: Arc::new(DagEngine::new()),
        cwd: startup.path().to_path_buf(),
        sessions,
        stores: Mutex::new(HashMap::new()),
        dirty: Arc::new(Notify::new()),
        task: Mutex::new(None),
    };

    handle.store_for(Some("sess-a")).await.unwrap();
    handle.store_for(Some("sess-b")).await.unwrap();
    handle.store_for(Some("sess-c")).await.unwrap();

    assert!(session_a
        .path()
        .join(".pi/graph-engineering-state-sess-a.db")
        .exists());
    assert!(session_b
        .path()
        .join(".pi/graph-engineering-state-sess-b.db")
        .exists());
    assert!(startup
        .path()
        .join(".pi/graph-engineering-state-sess-c.db")
        .exists());
    assert!(!startup
        .path()
        .join(".pi/graph-engineering-state-sess-a.db")
        .exists());
    assert_eq!(handle.stores.lock().len(), 3);
}

#[tokio::test]
async fn session_registry_manages_context_independent_of_bindings() {
    let work = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let ctx = test_context(&work.path().join("."), "s1", base.path()).await;
    let registry = SessionExecutionRegistry::new();

    assert!(registry.get_context("s1").is_none());
    assert!(registry.cwd_for("s1").is_none());
    assert!(registry.get("s1").is_none());
    registry.set_context("s1", ctx.clone());
    assert!(Arc::ptr_eq(&registry.get_context("s1").unwrap(), &ctx));
    assert_eq!(
        registry.cwd_for("s1").unwrap(),
        work.path().canonicalize().unwrap()
    );
    assert!(registry.get("s1").is_none());
    assert!(registry.remove("s1"));
    assert!(registry.get_context("s1").is_none());
    assert!(!registry.remove("s1"));
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
async fn flush_persists_two_cwd_owned_runs_only_under_owning_cwd() {
    let startup = tempfile::tempdir().unwrap();
    let session_a = tempfile::tempdir().unwrap();
    let session_b = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    let sessions = SessionExecutionRegistry::new();
    sessions.set_context(
        "sess-a".to_string(),
        test_context(session_a.path(), "sess-a", base.path()).await,
    );
    sessions.set_context(
        "sess-b".to_string(),
        test_context(session_b.path(), "sess-b", base.path()).await,
    );
    engine
        .plan(run_def("run-a"), None, Some("sess-a".into()))
        .unwrap();
    engine
        .plan(run_def("run-b"), None, Some("sess-b".into()))
        .unwrap();

    let handle = DagPersistHandle::spawn_with_sessions(
        engine.clone(),
        startup.path().to_path_buf(),
        sessions,
    );
    handle.flush().await;

    let cwd_a = session_a.path().canonicalize().unwrap();
    let cwd_b = session_b.path().canonicalize().unwrap();
    let runs_a = load_session_runs(&cwd_a, "sess-a").await;
    let runs_b = load_session_runs(&cwd_b, "sess-b").await;
    assert_eq!(runs_a.len(), 1);
    assert_eq!(runs_a[0].name, "run-a");
    assert_eq!(runs_a[0].session_id.as_deref(), Some("sess-a"));
    assert_eq!(runs_b.len(), 1);
    assert_eq!(runs_b[0].name, "run-b");
    assert_eq!(runs_b[0].session_id.as_deref(), Some("sess-b"));
    assert!(cwd_a
        .join(".pi/graph-engineering-state-sess-a.db")
        .exists());
    assert!(cwd_b
        .join(".pi/graph-engineering-state-sess-b.db")
        .exists());
    assert!(!cwd_a
        .join(".pi/graph-engineering-state-sess-b.db")
        .exists());
    assert!(!cwd_b
        .join(".pi/graph-engineering-state-sess-a.db")
        .exists());
    assert!(!startup
        .path()
        .join(".pi/graph-engineering-state-sess-a.db")
        .exists());
    assert!(!startup
        .path()
        .join(".pi/graph-engineering-state-sess-b.db")
        .exists());

    if let Some(task) = handle.task.lock().take() {
        task.abort();
    }
}

#[tokio::test]
async fn flush_removes_terminal_runs_from_the_session_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    let run = engine
        .plan(run_def("terminal-run"), None, Some("sess-1".into()))
        .unwrap();
    let handle = handle_without_task(engine.clone(), cwd.clone());
    handle.save_all().await.unwrap();
    assert_eq!(load_session_runs(&cwd, "sess-1").await.len(), 1);

    engine.cancel_run(&run.id, Some("test complete"));
    handle.save_all().await.unwrap();

    assert!(load_session_runs(&cwd, "sess-1").await.is_empty());
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
