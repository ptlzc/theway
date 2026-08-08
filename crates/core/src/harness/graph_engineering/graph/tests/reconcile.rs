use super::*;

#[test]
fn reconcile_readies_when_deps_succeed() {
    let def = run_def(
        "t",
        vec![
            node_def("a", "x", "t1", &[]),
            node_def("b", "x", "t2", &["a"]),
            node_def("c", "x", "t3", &["a", "b"]),
        ],
    );
    let mut run = build_run(&def);
    run.node_mut("a").unwrap().status = NodeStatus::Succeeded;
    reconcile(&mut run);
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Ready);
    // c: a done but b still non-terminal → pending.
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Pending);
    run.node_mut("b").unwrap().status = NodeStatus::Succeeded;
    reconcile(&mut run);
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Ready);
}

#[test]
fn reconcile_cancels_blocked_downstream() {
    let def = run_def(
        "t",
        vec![
            node_def("a", "x", "t1", &[]),
            node_def("b", "x", "t2", &["a"]),
            node_def("c", "x", "t3", &["b"]),
        ],
    );
    let mut run = build_run(&def);
    run.node_mut("a").unwrap().status = NodeStatus::Failed;
    reconcile(&mut run);
    let b = run.node("b").unwrap();
    assert_eq!(b.status, NodeStatus::Cancelled);
    assert_eq!(b.error.as_deref(), Some("blocked by a"));
    // Cancellation cascades within one pass.
    let c = run.node("c").unwrap();
    assert_eq!(c.status, NodeStatus::Cancelled);
    assert_eq!(c.error.as_deref(), Some("blocked by b"));
    assert!(c.completed_at.is_some());
}

#[test]
fn reconcile_treats_skipped_as_success_and_keeps_running() {
    let def = run_def(
        "t",
        vec![
            node_def("a", "x", "t1", &[]),
            node_def("b", "x", "t2", &["a"]),
            node_def("c", "x", "t3", &["b"]),
        ],
    );
    let mut run = build_run(&def);
    run.node_mut("a").unwrap().status = NodeStatus::Skipped;
    run.node_mut("b").unwrap().status = NodeStatus::Running;
    reconcile(&mut run);
    assert_eq!(run.node("b").unwrap().status, NodeStatus::Running);
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Pending);
    run.node_mut("b").unwrap().status = NodeStatus::Succeeded;
    reconcile(&mut run);
    assert_eq!(run.node("c").unwrap().status, NodeStatus::Ready);
}

#[test]
fn closure_includes_transitive_dependents() {
    let def = run_def(
        "t",
        vec![
            node_def("a", "x", "t", &[]),
            node_def("b", "x", "t", &["a"]),
            node_def("c", "x", "t", &["b"]),
            node_def("d", "x", "t", &["a"]),
        ],
    );
    let run = build_run(&def);
    let mut closure = downstream_closure(&run.nodes, "a");
    closure.sort_unstable();
    assert_eq!(closure, vec!["b", "c", "d"]);
}
