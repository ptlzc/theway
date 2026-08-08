use super::*;

#[test]
fn plan_auto_starts_roots() {
    let (engine, launcher) = engine_with_launcher();
    let def = run_def(
        "t",
        None,
        None,
        &[
            ("a", "explorer", "task a", &[]),
            ("b", "planner", "task b", &["a"]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    assert_eq!(run.id, "dag-1");
    assert_eq!(run.status, DagStatus::Running);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Running);
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Pending);
    assert_eq!(
        launcher.launched(),
        vec![("dag-1".to_string(), "a".to_string())]
    );
}

#[test]
fn plan_rejects_invalid_graph() {
    let engine = DagEngine::new();
    let def = run_def("t", None, None, &[("a", "x", "t", &["missing"])]);
    let err = engine.plan(def, None, None).unwrap_err();
    assert!(err.iter().any(|e| e.contains("missing")));
    assert!(engine.list_runs().is_empty());
}

#[test]
fn plan_without_launcher_fails_roots_immediately() {
    let engine = DagEngine::new();
    let def = run_def(
        "t",
        None,
        None,
        &[("a", "x", "t1", &[]), ("b", "x", "t2", &["a"])],
    );
    let run = engine.plan(def, None, None).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Failed);
    assert_eq!(
        run.node("a").unwrap().error.as_deref(),
        Some("no launch context")
    );
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(
        run.node("b").unwrap().error.as_deref(),
        Some("blocked by a")
    );
}
