use std::path::{Path, PathBuf};

use theway_contract::dag::{Direction, NodeStatus, PersistedRun, RunKind, state_path_for_project};

#[test]
fn persisted_snapshot_keeps_legacy_defaults_and_json_shape() {
    let json = r#"{
        "id":"dag-1","name":"legacy","maxConcurrency":2,"failFast":false,
        "direction":"TD","createdAt":1,"sessionId":null,
        "nodes":[{
            "id":"a","agent":"general","task":"work","dependsOn":[],
            "timeout":null,"cwd":null,"model":null,"thinking":null,
            "status":"running","attempt":1,"startedAt":2,"completedAt":null,
            "error":null,"inputTokens":null,"outputTokens":null,"result":null,
            "output":null,"livePreview":null
        }]
    }"#;

    let snapshot: PersistedRun = serde_json::from_str(json).unwrap();

    assert_eq!(snapshot.kind, RunKind::Dag);
    assert_eq!(snapshot.direction, Direction::Td);
    assert_eq!(snapshot.nodes[0].status, NodeStatus::Running);
    assert_eq!(snapshot.nodes[0].max_iterations, None);
    assert_eq!(snapshot.nodes[0].tools, None);
    let encoded = serde_json::to_value(snapshot).unwrap();
    assert_eq!(encoded["maxConcurrency"], 2);
    assert_eq!(encoded["nodes"][0]["dependsOn"], serde_json::json!([]));
}

#[test]
fn state_path_is_session_scoped_and_sanitized() {
    let pi = Path::new("/tmp/.pi");
    assert_eq!(
        state_path_for_project(pi, None),
        PathBuf::from("/tmp/.pi/graph-engineering-state.db")
    );
    assert_eq!(
        state_path_for_project(pi, Some("a/b c?d")),
        PathBuf::from("/tmp/.pi/graph-engineering-state-a_b_c_d.db")
    );
    assert_eq!(
        state_path_for_project(pi, Some("")),
        PathBuf::from("/tmp/.pi/graph-engineering-state-default.db")
    );
    assert_eq!(
        state_path_for_project(pi, Some(&"x".repeat(100)))
            .file_name()
            .unwrap(),
        format!("graph-engineering-state-{}.db", "x".repeat(60)).as_str()
    );
}
