use super::*;

#[test]
fn dependency_chain_advances_and_completes() {
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
    engine.on_node_completed(&id, "a", ok_outcome());
    assert_eq!(
        launcher.launched(),
        vec![(id.clone(), "a".to_string()), (id.clone(), "b".to_string())]
    );
    engine.on_node_completed(&id, "b", ok_outcome());
    assert_eq!(launcher.launched().len(), 3);
    assert_eq!(
        engine.get_run(&id).unwrap().node("c").unwrap().status,
        NodeStatus::Running
    );
    engine.on_node_completed(&id, "c", ok_outcome());
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    assert!(run.completed_at.is_some());
    assert_eq!(run.node("c").unwrap().output.as_deref(), Some("done"));
    assert_eq!(run.node("c").unwrap().input_tokens, Some(5));
    assert_eq!(run.node("c").unwrap().attempt, 1);
}

#[test]
fn tick_respects_concurrency_budget() {
    let (engine, launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        Some(1),
        None,
        &[("a", "x", "t1", &[]), ("b", "x", "t2", &[])],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    assert_eq!(launcher.launched(), vec![(id.clone(), "a".to_string())]);
    assert_eq!(
        engine.get_run(&id).unwrap().node("b").unwrap().status,
        NodeStatus::Ready
    );
    engine.on_node_completed(&id, "a", ok_outcome());
    assert_eq!(
        launcher.launched().last(),
        Some(&(id.clone(), "b".to_string()))
    );
}

#[test]
fn failfast_false_keeps_independent_branch() {
    let (engine, _launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        Some(false),
        &[
            ("a", "x", "t1", &[]),
            ("b", "x", "t2", &["a"]),
            ("c", "x", "t3", &[]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    engine.on_node_completed(&id, "a", fail_outcome("boom"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Running);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Failed);
    assert_eq!(run.node("a").unwrap().error.as_deref(), Some("boom"));
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(
        run.node("b").unwrap().error.as_deref(),
        Some("blocked by a")
    );
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Running);
    engine.on_node_completed(&id, "c", ok_outcome());
    assert_eq!(engine.get_run(&id).unwrap().status, DagStatus::Failed);
}

#[test]
fn failfast_true_cancels_whole_run() {
    let (engine, _launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        Some(true),
        &[
            ("a", "x", "t1", &[]),
            ("b", "x", "t2", &["a"]),
            ("c", "x", "t3", &[]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    engine.on_node_completed(&id, "a", fail_outcome("boom"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    let reason = run.error.clone().unwrap();
    assert!(reason.contains("failFast"));
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Failed);
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(
        run.node("c").unwrap().error.as_deref(),
        Some(reason.as_str())
    );
}

#[test]
fn on_node_update_syncs_tokens_preview_activity() {
    let (engine, _launcher) = engine_with_launcher();
    let run = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let id = run.id.clone();
    let launch_gen = engine.get_run(&id).unwrap().node("a").unwrap().launch_gen;
    let long_preview = "x".repeat(5000);
    engine.on_node_update(&id, "a", launch_gen, Some(100), Some(200), Some(long_preview));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("a").unwrap().input_tokens, Some(100));
    assert_eq!(run.node("a").unwrap().output_tokens, Some(200));
    assert_eq!(
        run.node("a")
            .unwrap()
            .live_preview
            .as_deref()
            .unwrap()
            .chars()
            .count(),
        2048
    );
    assert!(run.last_activity_at > 0);
}

#[test]
fn stale_launch_generation_update_is_dropped() {
    let (engine, _launcher) = engine_with_launcher();
    let run = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let id = run.id.clone();
    let first_gen = engine.get_run(&id).unwrap().node("a").unwrap().launch_gen;
    assert!(first_gen >= 1);
    // Fail the node so retry has a blocked node to reset and re-dispatch
    // (a fully-succeeded run is a retry no-op — TS parity).
    engine.on_node_completed(&id, "a", fail_outcome("boom"));
    assert_eq!(engine.get_run(&id).unwrap().status, DagStatus::Failed);
    assert_eq!(engine.retry(&id, None), vec!["a".to_string()]);
    let second_gen = engine.get_run(&id).unwrap().node("a").unwrap().launch_gen;
    assert!(second_gen > first_gen);
    // A stale job's update (pre-retry generation) must be dropped entirely —
    // tokens, preview, and the run's idle clock must not be touched.
    engine.on_node_update(&id, "a", first_gen, Some(999), Some(999), Some("stale".into()));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("a").unwrap().input_tokens, None);
    assert_eq!(run.node("a").unwrap().output_tokens, None);
    assert_eq!(run.node("a").unwrap().live_preview, None);
    // The current generation's update applies.
    engine.on_node_update(&id, "a", second_gen, Some(100), Some(200), Some("fresh".into()));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.node("a").unwrap().input_tokens, Some(100));
    assert_eq!(run.node("a").unwrap().live_preview.as_deref(), Some("fresh"));
}

#[test]
fn list_runs_order_and_most_recent_active() {
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
    let list = engine.list_runs();
    assert_eq!(list.len(), 2);
    assert!(list.windows(2).all(|w| w[0].created_at >= w[1].created_at));
    assert_eq!(engine.most_recent_active().unwrap().id, r2.id);
    assert_eq!(engine.running_node_count(), 1);
}
