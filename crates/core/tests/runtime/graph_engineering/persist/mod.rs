//! Tests for `persist` — split out of persist.rs (issue #11).

use super::*;

mod roundtrip;

pub(super) fn sample_run(id: &str, status: DagStatus) -> DagRun {
    let mk = |id: &str, status: NodeStatus, started: bool| DagNode {
        id: id.to_string(),
        agent: "explorer".to_string(),
        task: format!("task {id}"),
        depends_on: vec!["root".to_string()],
        timeout: Some(120),
        cwd: None,
        model: Some("m1".to_string()),
        thinking: Some("high".to_string()),
        status: status.clone(),
        job_id: Some("job-x".to_string()),
        attempt: 2,
        started_at: if started { Some(1000) } else { None },
        completed_at: None,
        error: None,
        input_tokens: Some(11),
        output_tokens: Some(22),
        result: None,
        output: Some("tail".to_string()),
        live_preview: Some("preview".to_string()),
        last_active_at: None,
    };
    DagRun {
        id: id.to_string(),
        name: format!("run {id}"),
        nodes: vec![
            mk("root", NodeStatus::Succeeded, true),
            mk("mid", NodeStatus::Running, true),
            mk("tail", NodeStatus::Pending, false),
        ],
        status,
        kind: RunKind::Dag,
        max_concurrency: 3,
        fail_fast: true,
        direction: Direction::Td,
        created_at: 500,
        session_id: Some("sess-1".to_string()),
        completed_at: None,
        last_activity_at: 900,
        error: None,
    }
}

pub(super) fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dag-persist-{name}-{}.json", std::process::id()))
}
