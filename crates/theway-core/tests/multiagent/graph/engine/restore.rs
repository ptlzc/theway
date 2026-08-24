use super::*;

#[test]
fn restore_hydrates_and_reschedules() {
    let (engine, launcher) = engine_with_launcher();
    let p = PersistedRun {
        id: "dag-7".to_string(),
        name: "resumed".to_string(),
        max_concurrency: 2,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 100,
        session_id: Some("sess".to_string()),
        kind: RunKind::Dag,
        nodes: vec![
            persisted_node("a", NodeStatus::Running, &[]),
            persisted_node("b", NodeStatus::Pending, &["a"]),
        ],
    };
    let restored = engine.restore(vec![p]);
    assert_eq!(restored, vec!["dag-7".to_string()]);
    let run = engine.get_run("dag-7").unwrap();
    assert_eq!(run.status, DagStatus::Running);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Running);
    assert_eq!(
        run.node("a").unwrap().job_id,
        Some("job-dag-7-a".to_string())
    );
    assert_eq!(
        launcher.launched(),
        vec![("dag-7".to_string(), "a".to_string())]
    );
    engine.on_node_completed("dag-7", "a", ok_outcome());
    assert_eq!(
        engine.get_run("dag-7").unwrap().node("b").unwrap().status,
        NodeStatus::Running
    );
    // Duplicate restore of the same id is skipped.
    let dup = PersistedRun {
        id: "dag-7".to_string(),
        name: "dup".to_string(),
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 100,
        session_id: None,
        kind: RunKind::Dag,
        nodes: vec![persisted_node("a", NodeStatus::Running, &[])],
    };
    assert!(engine.restore(vec![dup]).is_empty());
}

#[test]
fn restored_runs_dispatch_through_session_launcher_registered_before_restore() {
    let engine = DagEngine::new();
    let session_launcher = Arc::new(FakeLauncher::new());
    engine.set_session_launcher(Some("sess".to_string()), session_launcher.clone());
    let p = PersistedRun {
        id: "dag-7".to_string(),
        name: "resumed".to_string(),
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 100,
        session_id: Some("sess".to_string()),
        kind: RunKind::Dag,
        nodes: vec![persisted_node("a", NodeStatus::Running, &[])],
    };

    let restored = engine.restore(vec![p]);

    assert_eq!(restored, vec!["dag-7".to_string()]);
    assert_eq!(
        session_launcher.launched(),
        vec![("dag-7".to_string(), "a".to_string())]
    );
}

#[test]
fn restore_aligns_dag_counter() {
    let (engine, _launcher) = engine_with_launcher();
    let p = PersistedRun {
        id: "dag-12".to_string(),
        name: "resumed".to_string(),
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 100,
        session_id: None,
        kind: RunKind::Dag,
        nodes: vec![persisted_node("a", NodeStatus::Running, &[])],
    };
    engine.restore(vec![p]);
    let run = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    assert_eq!(run.id, "dag-13");
}
