//! DAG engine lifecycle edge cases.

use std::sync::Arc;

use super::super::*;
use crate::multiagent::graph::engine::NodeLauncher;
use crate::multiagent::graph::types::DagNodeDef;
use tokio_util::sync::CancellationToken;

fn node_def(id: &str) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "x".to_string(),
        task: format!("task {id}"),
        depends_on: None,
        timeout: None,
        cwd: None,
        provider: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(name: &str) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: vec![node_def("a")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

struct FakeLauncher;

impl NodeLauncher for FakeLauncher {
    fn launch(&self, _run_id: &str, _node_id: &str, _cancel: CancellationToken) {}
}

#[test]
fn debug_fmt_reports_engine_state() {
    let engine = DagEngine::new();
    let text = format!("{engine:?}");
    assert!(text.contains("DagEngine"));
    assert!(text.contains("runs: 0"));
}

#[test]
fn plan_rejects_unknown_agent_when_known_agents_supplied() {
    let engine = DagEngine::new();
    let known = vec!["explorer".to_string()];
    let err = engine
        .plan(run_def("t"), Some(&known), None)
        .unwrap_err();
    assert!(err.iter().any(|e| e.contains("未知 subagent \"x\"")));
    assert!(engine.list_runs().is_empty());
}

#[test]
fn most_recent_active_none_when_no_running() {
    let engine = DagEngine::new();
    assert!(engine.most_recent_active().is_none());

    let run = engine.plan(run_def("t"), None, None).unwrap();
    // No launcher: root fails immediately and the run is terminal.
    assert_eq!(engine.get_run(&run.id).unwrap().status, DagStatus::Failed);
    assert!(engine.most_recent_active().is_none());
}

#[test]
fn on_goal_tick_missing_main_node_returns_false() {
    let engine = DagEngine::new();
    // Insert a run with no nodes directly, then on_goal_tick must return false.
    engine.inner.lock().runs.insert(
        "goal-empty".into(),
        DagRun {
            id: "goal-empty".into(),
            name: "empty".into(),
            nodes: Vec::new(),
            status: DagStatus::Running,
            kind: RunKind::Goal,
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

    assert!(!engine.on_goal_tick("goal-empty", 1, false, None));
}

#[test]
fn on_goal_evaluator_finished_unknown_or_missing_main_is_noop() {
    let engine = DagEngine::new();
    engine.on_goal_evaluator_finished("missing", "job-1".into());

    engine.inner.lock().runs.insert(
        "goal-empty".into(),
        DagRun {
            id: "goal-empty".into(),
            name: "empty".into(),
            nodes: Vec::new(),
            status: DagStatus::Running,
            kind: RunKind::Goal,
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
    engine.on_goal_evaluator_finished("goal-empty", "job-1".into());
}

#[test]
fn cancel_run_unknown_or_terminal_is_noop() {
    let engine = DagEngine::new();
    engine.cancel_run("missing", Some("noop"));

    let run = engine.plan(run_def("t"), None, None).unwrap();
    assert_eq!(engine.get_run(&run.id).unwrap().status, DagStatus::Failed);
    engine.cancel_run(&run.id, Some("late"));
    assert_eq!(engine.get_run(&run.id).unwrap().status, DagStatus::Failed);
}

#[test]
fn retry_unknown_run_or_node_is_noop() {
    let engine = DagEngine::new();
    assert!(engine.retry("missing", None).is_empty());

    engine.set_launcher(Some(Arc::new(FakeLauncher)));
    let run = engine.plan(run_def("t"), None, None).unwrap();
    assert!(engine.retry(&run.id, Some(&["missing-node".to_string()])).is_empty());
}

#[test]
fn skip_unknown_run_or_node_returns_false() {
    let engine = DagEngine::new();
    assert!(!engine.skip("missing", "a"));

    let run = engine.plan(run_def("t"), None, None).unwrap();
    assert!(!engine.skip(&run.id, "missing-node"));
}

#[test]
fn abort_all_runs_returns_zero_when_none_running() {
    let engine = DagEngine::new();
    assert_eq!(engine.abort_all_runs("shutdown"), 0);
}

#[test]
fn complete_goal_unknown_run_is_noop() {
    let engine = DagEngine::new();
    engine.complete_goal("missing", DagStatus::Cancelled, Some("x".to_string()));
}

#[test]
fn list_runs_orders_newest_first() {
    let engine = DagEngine::new();
    engine.set_launcher(Some(Arc::new(FakeLauncher)));
    let r1 = engine.plan(run_def("t1"), None, None).unwrap();
    let r2 = engine.plan(run_def("t2"), None, None).unwrap();

    // Distinct creation times: newest (r2) must sort before r1.
    engine.inner.lock().runs.get_mut(&r1.id).unwrap().created_at = 100;
    engine.inner.lock().runs.get_mut(&r2.id).unwrap().created_at = 200;

    let list = engine.list_runs();
    let pos1 = list.iter().position(|r| r.id == r1.id).unwrap();
    let pos2 = list.iter().position(|r| r.id == r2.id).unwrap();
    assert!(pos2 < pos1, "newest run must sort first");
}

#[test]
fn rehome_runs_without_matching_session_returns_zero() {
    let engine = DagEngine::new();
    engine.set_launcher(Some(Arc::new(FakeLauncher)));
    let run = engine
        .plan(run_def("t"), None, Some("session-a".into()))
        .unwrap();

    assert_eq!(engine.rehome_runs("session-b", "session-c"), 0);
    assert_eq!(engine.get_run(&run.id).unwrap().session_id, Some("session-a".into()));
}

#[test]
fn cancel_run_with_running_node_without_job_token_is_noop_for_jobs() {
    let engine = DagEngine::new();
    engine.inner.lock().runs.insert(
        "run".into(),
        DagRun {
            id: "run".into(),
            name: "t".into(),
            nodes: vec![DagNode {
                id: "a".into(),
                agent: "x".into(),
                task: "task".into(),
                depends_on: Vec::new(),
                timeout: None,
                cwd: None,
                provider: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
                status: NodeStatus::Running,
                job_id: None,
                attempt: 0,
                launch_gen: 0,
                started_at: Some(1),
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
            kind: RunKind::Dag,
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

    engine.cancel_run("run", Some("shutdown"));

    let run = engine.get_run("run").unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Cancelled);
}

#[test]
fn retry_with_empty_explicit_ids_uses_all_blocked_nodes() {
    let (engine, _launcher) = super::engine_with_launcher();
    let def = super::run_def(
        "t",
        None,
        None,
        &[
            ("a", "x", "t1", &[]),
            ("b", "x", "t2", &["a"]),
        ],
    );
    let run = engine.plan(def, None, None).unwrap();
    let id = run.id.clone();
    engine.on_node_completed(&id, "a", super::fail_outcome("boom"));
    assert_eq!(
        engine.get_run(&id).unwrap().node("b").unwrap().status,
        NodeStatus::Cancelled
    );

    let reset = engine.retry(&id, Some(&[]));

    assert_eq!(reset, vec!["a".to_string(), "b".to_string()]);
}
