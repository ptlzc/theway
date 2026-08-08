use super::*;

#[test]
fn retry_resets_blocked_nodes_with_closure() {
    let (engine, _launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        None,
        &[
            ("a", "x", "t1", &[]),
            ("b", "x", "t2", &["a"]),
            ("c", "x", "t3", &["b"]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    engine.on_node_completed(&id, "a", fail_outcome("boom"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Cancelled);
    let reset = engine.retry(&id, None);
    assert_eq!(
        reset,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Running);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Running);
    assert_eq!(run.node("a").unwrap().attempt, 0);
    assert_eq!(run.node("a").unwrap().error, None);
    engine.on_node_completed(&id, "a", ok_outcome());
    assert_eq!(
        engine.get_run(&id).unwrap().node("b").unwrap().status,
        NodeStatus::Running
    );
    engine.on_node_completed(&id, "b", ok_outcome());
    assert_eq!(
        engine.get_run(&id).unwrap().node("c").unwrap().status,
        NodeStatus::Running
    );
    engine.on_node_completed(&id, "c", ok_outcome());
    assert_eq!(engine.get_run(&id).unwrap().status, DagStatus::Completed);
}

#[test]
fn retry_explicit_ids_reset_only_closure() {
    let (engine, _launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        None,
        &[
            ("a", "x", "t1", &[]),
            ("b", "x", "t2", &["a"]),
            ("d", "x", "t4", &[]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    engine.on_node_completed(&id, "a", fail_outcome("boom"));
    engine.on_node_completed(&id, "d", ok_outcome());
    let reset = engine.retry(&id, Some(&["a".to_string()]));
    assert_eq!(reset, vec!["a".to_string(), "b".to_string()]);
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Running);
    assert_eq!(run.node("d").unwrap().status, NodeStatus::Succeeded);
    assert_eq!(
        engine.get_run(&id).unwrap().node("b").unwrap().status,
        NodeStatus::Pending
    );
    engine.on_node_completed(&id, "a", ok_outcome());
    assert_eq!(
        engine.get_run(&id).unwrap().node("b").unwrap().status,
        NodeStatus::Running
    );
    engine.on_node_completed(&id, "b", ok_outcome());
    assert_eq!(engine.get_run(&id).unwrap().status, DagStatus::Completed);
}

#[test]
fn skip_failed_releases_downstream_closure() {
    let (engine, launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        None,
        &[
            ("a", "x", "t1", &[]),
            ("b", "x", "t2", &["a"]),
            ("c", "x", "t3", &["b"]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    engine.on_node_completed(&id, "a", fail_outcome("boom"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Cancelled);
    assert!(engine.skip(&id, "a"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Skipped);
    assert_eq!(
        run.node("a").unwrap().error.as_deref(),
        Some("skipped by orchestrator")
    );
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Running);
    assert_eq!(
        launcher.launched().last(),
        Some(&(id.clone(), "b".to_string()))
    );
    engine.on_node_completed(&id, "b", ok_outcome());
    assert_eq!(
        engine.get_run(&id).unwrap().node("c").unwrap().status,
        NodeStatus::Running
    );
    engine.on_node_completed(&id, "c", ok_outcome());
    assert_eq!(engine.get_run(&id).unwrap().status, DagStatus::Completed);
    // Skipping an already-succeeded node is a no-op.
    assert!(!engine.skip(&id, "c"));
}

#[test]
fn skip_running_aborts_job() {
    let (engine, launcher) = engine_with_launcher();
    let def = run_def("t", None, None, &[("a", "x", "t1", &[])]);
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    let token = launcher.tokens().pop().unwrap();
    assert!(!token.is_cancelled());
    assert!(engine.skip(&id, "a"));
    assert!(token.is_cancelled());
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Skipped);
    assert_eq!(
        run.node("a").unwrap().error.as_deref(),
        Some("skipped by orchestrator (job aborted)")
    );
    assert_eq!(run.status, DagStatus::Completed);
    // Stale completion after skip is dropped.
    engine.on_node_completed(&id, "a", ok_outcome());
    assert_eq!(
        engine.get_run(&id).unwrap().node("a").unwrap().status,
        NodeStatus::Skipped
    );
}
