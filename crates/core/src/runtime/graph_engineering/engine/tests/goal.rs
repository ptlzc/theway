use super::*;

#[test]
fn plan_goal_creates_single_node_run() {
    let engine = DagEngine::new();
    let id = engine.plan_goal("finish the migration", Some("sess-1".to_string()));
    assert!(id.starts_with("goal-"));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.kind, RunKind::Goal);
    assert_eq!(run.status, DagStatus::Running);
    assert_eq!(run.name, "finish the migration"); // < 48 chars, no truncation
    assert_eq!(run.session_id.as_deref(), Some("sess-1"));
    assert!(run.completed_at.is_none());
    assert_eq!(run.nodes.len(), 1);
    let node = &run.nodes[0];
    assert_eq!(node.id, "main");
    assert_eq!(node.agent, "main-agent");
    assert_eq!(node.task, "finish the migration");
    assert_eq!(node.status, NodeStatus::Running);
    assert_eq!(node.attempt, 0);
    assert!(node.depends_on.is_empty());
    // goal runs count as running nodes but never touch the launcher.
    assert_eq!(engine.running_node_count(), 1);

    // Long conditions are truncated to 48 chars.
    let long = "x".repeat(100);
    let id2 = engine.plan_goal(&long, None);
    assert_eq!(engine.get_run(&id2).unwrap().name.chars().count(), 48);
}

#[test]
fn on_goal_tick_updates_iteration_and_completes() {
    let engine = DagEngine::new();
    let id = engine.plan_goal("loop until done", None);

    assert!(engine.on_goal_tick(&id, 1, false, Some("not yet".to_string())));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Running);
    assert!(run.completed_at.is_none());
    let node = run.node("main").unwrap();
    assert_eq!(node.status, NodeStatus::Running);
    assert_eq!(node.attempt, 1);
    assert_eq!(node.error.as_deref(), Some("not yet"));

    assert!(engine.on_goal_tick(&id, 2, true, None));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    assert!(run.completed_at.is_some());
    let node = run.node("main").unwrap();
    assert_eq!(node.status, NodeStatus::Succeeded);
    assert_eq!(node.attempt, 2);
    assert_eq!(node.error, None);

    // Unknown run → false.
    assert!(!engine.on_goal_tick("goal-999", 1, false, None));
}

#[test]
fn complete_goal_cancels_run() {
    let engine = DagEngine::new();
    let id = engine.plan_goal("loop", None);
    engine.complete_goal(&id, DagStatus::Cancelled, Some("user abort".to_string()));
    let run = engine.get_run(&id).unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    assert!(run.completed_at.is_some());
    assert_eq!(run.error.as_deref(), Some("user abort"));
    let node = run.node("main").unwrap();
    assert_eq!(node.status, NodeStatus::Cancelled);
    assert_eq!(node.error.as_deref(), Some("user abort"));

    // Failed variant mirrors the run status onto the node too.
    let id2 = engine.plan_goal("loop2", None);
    engine.complete_goal(
        &id2,
        DagStatus::Failed,
        Some("condition broken".to_string()),
    );
    let run = engine.get_run(&id2).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    assert_eq!(run.node("main").unwrap().status, NodeStatus::Failed);
    assert_eq!(
        run.node("main").unwrap().error.as_deref(),
        Some("condition broken")
    );

    // Already-terminal runs are left alone.
    engine.complete_goal(&id2, DagStatus::Cancelled, Some("late cancel".to_string()));
    assert_eq!(engine.get_run(&id2).unwrap().status, DagStatus::Failed);
}

#[test]
fn goal_run_emits_events() {
    let engine = DagEngine::new();
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    engine.set_event_sender(Some(tx));

    // plan_goal → RunStatus running.
    let id = engine.plan_goal("loop", None);
    match rx.try_recv().unwrap() {
        DagEvent::RunStatus {
            run_id,
            status,
            error,
            ..
        } => {
            assert_eq!(run_id, id);
            assert_eq!(status, DagStatus::Running);
            assert_eq!(error, None);
        }
        other => panic!("expected RunStatus running, got {other:?}"),
    }

    // Tick, not done → NodeStatus running with the reason.
    engine.on_goal_tick(&id, 1, false, Some("keep going".to_string()));
    match rx.try_recv().unwrap() {
        DagEvent::NodeStatus {
            run_id,
            node_id,
            status,
            error,
            ..
        } => {
            assert_eq!(run_id, id);
            assert_eq!(node_id, "main");
            assert_eq!(status, NodeStatus::Running);
            assert_eq!(error.as_deref(), Some("keep going"));
        }
        other => panic!("expected NodeStatus running, got {other:?}"),
    }

    // Tick, done → NodeStatus succeeded + RunStatus completed.
    engine.on_goal_tick(&id, 2, true, None);
    match rx.try_recv().unwrap() {
        DagEvent::NodeStatus { status, .. } => assert_eq!(status, NodeStatus::Succeeded),
        other => panic!("expected NodeStatus succeeded, got {other:?}"),
    }
    match rx.try_recv().unwrap() {
        DagEvent::RunStatus { status, .. } => assert_eq!(status, DagStatus::Completed),
        other => panic!("expected RunStatus completed, got {other:?}"),
    }

    // complete_goal → NodeStatus + RunStatus.
    let id2 = engine.plan_goal("loop2", None);
    match rx.try_recv().unwrap() {
        DagEvent::RunStatus { run_id, .. } => assert_eq!(run_id, id2),
        other => panic!("expected RunStatus running for goal-2, got {other:?}"),
    }
    engine.complete_goal(&id2, DagStatus::Cancelled, Some("cancel".to_string()));
    match rx.try_recv().unwrap() {
        DagEvent::NodeStatus { status, error, .. } => {
            assert_eq!(status, NodeStatus::Cancelled);
            assert_eq!(error.as_deref(), Some("cancel"));
        }
        other => panic!("expected NodeStatus cancelled, got {other:?}"),
    }
    match rx.try_recv().unwrap() {
        DagEvent::RunStatus { status, error, .. } => {
            assert_eq!(status, DagStatus::Cancelled);
            assert_eq!(error.as_deref(), Some("cancel"));
        }
        other => panic!("expected RunStatus cancelled, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "no further events expected");

    // Detach (None) → further transitions emit nothing.
    engine.set_event_sender(None);
    let id3 = engine.plan_goal("loop3", None);
    assert!(rx.try_recv().is_err());
    engine.on_goal_tick(&id3, 1, true, None);
    assert!(rx.try_recv().is_err());
}

#[test]
fn goal_counter_independent_of_dag_counter() {
    let engine = DagEngine::new();
    let g1 = engine.plan_goal("g1", None);
    let g2 = engine.plan_goal("g2", None);
    assert_eq!(g1, "goal-1");
    assert_eq!(g2, "goal-2");

    // dag-N numbering is untouched by goal runs (and vice versa).
    let def = run_def("t", None, None, &[("a", "x", "t", &[])]);
    let dag = engine.plan(def, None, None).unwrap();
    assert_eq!(dag.id, "dag-1");
    assert_eq!(dag.kind, RunKind::Dag);
}
