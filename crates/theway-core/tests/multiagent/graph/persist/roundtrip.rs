use super::*;

use crate::multiagent::graph::model::build_run;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef};

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
