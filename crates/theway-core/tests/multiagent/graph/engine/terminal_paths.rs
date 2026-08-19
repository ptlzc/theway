//! DAG goal completion and terminal transition paths.

use super::super::*;

fn insert_run(engine: &DagEngine, id: &str, node_id: &str, status: NodeStatus, kind: RunKind) {
    engine.inner.lock().runs.insert(
        id.to_string(),
        DagRun {
            id: id.to_string(),
            name: "linecov".to_string(),
            nodes: vec![DagNode {
                id: node_id.to_string(),
                agent: "x".to_string(),
                task: "task".to_string(),
                depends_on: Vec::new(),
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
                status,
                job_id: None,
                attempt: 0,
                launch_gen: 0,
                started_at: None,
                completed_at: None,
                error: None,
                input_tokens: None,
                output_tokens: None,
                result: None,
                output: None,
                live_preview: None,
                last_active_at: None,
            }],
            status: DagStatus::Running,
            kind,
            max_concurrency: 1,
            fail_fast: false,
            direction: Direction::Td,
            created_at: 1,
            session_id: None,
            completed_at: None,
            last_activity_at: 1,
            error: None,
        },
    );
}

#[test]
fn complete_goal_non_failed_cancelled_preserves_main_status() {
    let engine = DagEngine::new();
    insert_run(&engine, "goal-main", "main", NodeStatus::Running, RunKind::Goal);

    engine.complete_goal("goal-main", DagStatus::Completed, None);

    let run = engine.get_run("goal-main").unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    assert_eq!(run.node("main").unwrap().status, NodeStatus::Running);
}

#[test]
fn complete_goal_without_main_node_still_completes_run() {
    let engine = DagEngine::new();
    insert_run(&engine, "goal-no-main", "a", NodeStatus::Running, RunKind::Dag);

    engine.complete_goal("goal-no-main", DagStatus::Completed, None);

    let run = engine.get_run("goal-no-main").unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Running);
}

#[test]
fn retry_skips_non_blocked_targets() {
    let engine = DagEngine::new();
    insert_run(&engine, "retry-nonblocked", "a", NodeStatus::Running, RunKind::Dag);

    let reset = engine.retry("retry-nonblocked", Some(&["a".to_string()]));

    assert!(reset.is_empty());
}
