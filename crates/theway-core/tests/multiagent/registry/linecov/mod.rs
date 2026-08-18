//! Additional line-coverage tests for `multiagent::registry` (see docs/rust-test-files.md).

use super::super::*;

#[test]
fn job_messages_returns_in_memory_messages_when_present() {
    let registry = AgentJobRegistry::new();
    let id = registry.register(JobInit {
        agent: "tester".into(),
        source: "dag".into(),
        run_id: Some("run-1".into()),
        node_id: Some("a".into()),
        session_id: None,
    });
    registry.update(&id, |job| {
        job.messages.push(serde_json::json!({"text": "hello"}));
    });

    let messages = registry.job_messages(&id).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["text"], serde_json::json!("hello"));
}
