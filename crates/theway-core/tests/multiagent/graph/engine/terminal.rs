use super::*;

#[test]
fn cancel_run_aborts_jobs_and_drops_stale_reports() {
    let (engine, launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        None,
        &[("a", "x", "t1", &[]), ("b", "x", "t2", &[])],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    assert_eq!(launcher.launched().len(), 2);
    let tokens = launcher.tokens();
    assert!(tokens.iter().all(|t| !t.is_cancelled()));
    engine.cancel_run(&id, Some("session shutdown"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    assert_eq!(run.error.as_deref(), Some("session shutdown"));
    assert!(run.nodes.iter().all(|n| n.status == NodeStatus::Cancelled));
    assert!(tokens.iter().all(|t| t.is_cancelled()));
    // Stale completion report after cancel is ignored.
    engine.on_node_completed(&id, "a", ok_outcome());
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(run.status, DagStatus::Cancelled);
}

#[test]
fn abort_all_runs_cancels_running_only() {
    let (engine, _launcher) = engine_with_launcher();
    let r1 = engine
        .plan(
            run_def("t1", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let r2 = engine
        .plan(
            run_def("t2", None, None, &[("b", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    engine.on_node_completed(&r1.id, "a", ok_outcome());
    assert_eq!(engine.abort_all_runs("session shutdown"), 1);
    assert_eq!(engine.get_run(&r1.id).unwrap().status, DagStatus::Completed);
    assert_eq!(engine.get_run(&r2.id).unwrap().status, DagStatus::Cancelled);
}

#[test]
fn reset_for_tests_clears_state() {
    let (engine, _launcher) = engine_with_launcher();
    let r1 = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    assert_eq!(r1.id, "dag-1");
    engine.__reset_for_tests();
    assert!(engine.list_runs().is_empty());
    assert_eq!(engine.running_node_count(), 0);
    let r2 = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    assert_eq!(r2.id, "dag-1");
}
