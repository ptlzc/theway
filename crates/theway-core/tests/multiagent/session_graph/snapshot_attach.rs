use crate::multiagent::graph::engine::DagEngine;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef};
use crate::multiagent::jobs::{SubagentJobInit, SubagentJobRegistry};
use crate::multiagent::session_graph::{attach_runs, snapshot_for_session};

fn run_def() -> DagRunDef {
    DagRunDef {
        name: "snapshot".into(),
        nodes: vec![DagNodeDef {
            id: "a".into(),
            agent: "general".into(),
            task: "do work".into(),
            depends_on: None,
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

#[test]
fn snapshot_for_session_filters_engine_and_jobs_by_session() {
    // Arrange
    let engine = DagEngine::new();
    let jobs = SubagentJobRegistry::new();
    let session_id = "session-a";
    let run = engine.plan(run_def(), None, Some(session_id.into())).unwrap();
    let job_id = jobs.register(SubagentJobInit {
        agent: "general".into(),
        source: "dag".into(),
        run_id: Some(run.id.clone()),
        node_id: Some("a".into()),
        session_id: Some(session_id.into()),
    });

    // Act
    let state = snapshot_for_session(&engine, &jobs, Some(session_id));

    // Assert
    assert_eq!(state.dags.len(), 1);
    assert_eq!(state.dags[0].id, run.id);
    assert_eq!(state.subagents.len(), 1);
    assert_eq!(state.subagents[0].id, job_id);
}

#[test]
fn snapshot_for_session_excludes_other_sessions() {
    // Arrange
    let engine = DagEngine::new();
    let jobs = SubagentJobRegistry::new();
    engine.plan(run_def(), None, Some("session-a".into())).unwrap();
    jobs.register(SubagentJobInit {
        agent: "general".into(),
        source: "dag".into(),
        run_id: None,
        node_id: None,
        session_id: Some("session-a".into()),
    });

    // Act
    let state = snapshot_for_session(&engine, &jobs, Some("session-b"));

    // Assert
    assert!(state.dags.is_empty());
    assert!(state.subagents.is_empty());
}

#[test]
fn attach_runs_moves_engine_runs_and_job_ownership() {
    // Arrange
    let engine = DagEngine::new();
    let jobs = SubagentJobRegistry::new();
    let run = engine.plan(run_def(), None, Some("old".into())).unwrap();
    let job_id = jobs.register(SubagentJobInit {
        agent: "general".into(),
        source: "dag".into(),
        run_id: Some(run.id.clone()),
        node_id: Some("a".into()),
        session_id: Some("old".into()),
    });

    // Act
    let moved = attach_runs(&engine, &jobs, "old", "new");

    // Assert
    assert_eq!(moved, 2);
    assert_eq!(engine.get_run(&run.id).unwrap().session_id.as_deref(), Some("new"));
    assert_eq!(jobs.job(&job_id).unwrap().session_id.as_deref(), Some("new"));
}
