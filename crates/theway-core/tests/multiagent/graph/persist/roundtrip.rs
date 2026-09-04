use super::*;

use crate::multiagent::graph::model::build_run;
use crate::multiagent::graph::types::{
    DagNodeDef, DagRunDef, DagStatus, Direction, NodeResult, NodeStatus, RunKind,
};

fn budgeted_run() -> DagRun {
    let def = DagRunDef {
        name: "budget".to_string(),
        nodes: vec![
            DagNodeDef {
                id: "a".into(),
                agent: "general".into(),
                task: "t".into(),
                depends_on: None,
                timeout: None,
                cwd: None,
                provider: None,
                model: None,
                thinking: None,
                max_iterations: Some(12),
                tools: Some(vec!["read".into(), "bash".into()]),
            },
            DagNodeDef {
                id: "b".into(),
                agent: "general".into(),
                task: "t2".into(),
                depends_on: Some(vec!["a".into()]),
                timeout: None,
                cwd: None,
                provider: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
            },
        ],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    build_run(&def)
}

#[test]
fn snapshot_hydrate_roundtrip_preserves_budget_and_tools() {
    let run = budgeted_run();
    let restored = hydrate(to_persisted(&run));

    let a = restored.node("a").unwrap();
    assert_eq!(a.max_iterations, Some(12));
    assert_eq!(a.tools, Some(vec!["read".into(), "bash".into()]));

    let b = restored.node("b").unwrap();
    assert_eq!(b.max_iterations, None);
    assert_eq!(b.tools, None);
}

#[test]
fn snapshot_json_uses_camel_case_budget_keys() {
    let value = serde_json::to_value(to_persisted(&budgeted_run())).unwrap();
    let node = &value["nodes"][0];
    assert_eq!(node["maxIterations"], 12);
    assert_eq!(node["tools"], serde_json::json!(["read", "bash"]));
}

#[test]
fn legacy_state_without_budget_fields_hydrates_none() {
    // Shape written before the budget/allowlist fields existed: the keys are
    // absent entirely, so the `#[serde(default)]` projections must apply.
    let json = r#"{
        "id": "dag-1",
        "name": "legacy",
        "maxConcurrency": 10,
        "failFast": false,
        "direction": "TD",
        "createdAt": 0,
        "sessionId": null,
        "nodes": [{
            "id": "a",
            "agent": "general",
            "task": "t",
            "dependsOn": [],
            "timeout": null,
            "cwd": null,
            "model": null,
            "thinking": null,
            "status": "pending",
            "attempt": 0,
            "startedAt": null,
            "completedAt": null,
            "error": null,
            "inputTokens": null,
            "outputTokens": null,
            "result": null,
            "output": null,
            "livePreview": null
        }]
    }"#;
    let persisted: PersistedRun = serde_json::from_str(json).unwrap();
    let run = hydrate(persisted);
    let node = run.node("a").unwrap();
    assert_eq!(node.max_iterations, None);
    assert_eq!(node.tools, None);
}

#[test]
fn hydrate_demotes_running_nodes_and_preserves_terminal_results() {
    let snapshot = PersistedRun {
        id: "dag-9".into(),
        name: "restore".into(),
        max_concurrency: 2,
        fail_fast: false,
        direction: Direction::Lr,
        created_at: 1,
        session_id: None,
        kind: RunKind::Dag,
        nodes: vec![
            PersistedNode {
                id: "a".into(),
                agent: "x".into(),
                task: "t".into(),
                depends_on: vec![],
                timeout: None,
                cwd: None,
                provider: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
                status: NodeStatus::Running,
                attempt: 1,
                started_at: Some(42),
                completed_at: None,
                error: None,
                input_tokens: Some(3),
                output_tokens: None,
                result: None,
                output: None,
                live_preview: Some("live".into()),
            },
            PersistedNode {
                id: "b".into(),
                agent: "x".into(),
                task: "t".into(),
                depends_on: vec!["a".into()],
                timeout: None,
                cwd: None,
                provider: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
                status: NodeStatus::Succeeded,
                attempt: 3,
                started_at: Some(10),
                completed_at: Some(20),
                error: None,
                input_tokens: Some(1),
                output_tokens: Some(2),
                result: Some(NodeResult {
                    success: true,
                    error: None,
                    duration_ms: Some(5),
                    attempt: 3,
                    total_attempts: 3,
                }),
                output: Some("out".into()),
                live_preview: None,
            },
        ],
    };

    let run = hydrate(snapshot);

    assert_eq!(run.status, DagStatus::Running);
    assert!(run.last_activity_at > 0);
    let running = run.node("a").unwrap();
    assert_eq!(running.status, NodeStatus::Ready);
    assert_eq!(running.started_at, None);
    assert_eq!(running.job_id, None);
    assert_eq!(running.live_preview.as_deref(), Some("live"));
    let succeeded = run.node("b").unwrap();
    assert_eq!(succeeded.status, NodeStatus::Succeeded);
    assert_eq!(succeeded.started_at, Some(10));
    assert_eq!(succeeded.completed_at, Some(20));
    assert!(succeeded.result.is_some());
    assert_eq!(run.direction, Direction::Lr);
    assert_eq!(run.created_at, 1);
}

#[test]
fn session_graph_state_projection_roundtrips_persisted_runs() {
    // Arrange
    let run = budgeted_run();
    let persisted = to_persisted(&run);

    // Act
    let state = to_session_graph_state(vec![persisted.clone()]);
    let restored = from_session_graph_state(&state);

    // Assert
    assert_eq!(restored, vec![persisted]);
    assert!(state.subagents.is_empty());
}

#[test]
fn max_run_counter_uses_only_well_formed_dag_ids() {
    let run = |id: &str| DagRun {
        id: id.into(),
        name: String::new(),
        nodes: vec![],
        status: DagStatus::Running,
        kind: RunKind::Dag,
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 0,
        session_id: None,
        completed_at: None,
        last_activity_at: 0,
        error: None,
    };

    assert_eq!(max_run_counter(&[]), 0);
    assert_eq!(
        max_run_counter(&[run("dag-1"), run("dag-7"), run("dag-3")]),
        7
    );
    assert_eq!(
        max_run_counter(&[run("run-2"), run("dag-12x"), run("x-dag-4")]),
        0
    );
}
