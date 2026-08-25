//! Registry message lookup behavior.

use std::sync::Arc;

use super::super::*;

#[derive(Default)]
struct MemoryTranscriptStore {
    nodes:
        parking_lot::Mutex<std::collections::HashMap<(String, String), Vec<serde_json::Value>>>,
    jobs: parking_lot::Mutex<std::collections::HashMap<String, Vec<serde_json::Value>>>,
}

impl JobTranscriptStore for MemoryTranscriptStore {
    fn save(&self, transcript: &JobTranscript) {
        let messages = transcript.messages.to_vec();
        match (transcript.run_id, transcript.node_id) {
            (Some(run), Some(node)) => {
                self.nodes
                    .lock()
                    .insert((run.to_string(), node.to_string()), messages);
            }
            _ => {
                self.jobs
                    .lock()
                    .insert(transcript.job_id.to_string(), messages);
            }
        }
    }

    fn load_node(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
        self.nodes
            .lock()
            .get(&(run_id.to_string(), node_id.to_string()))
            .cloned()
    }

    fn load_job(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
        self.jobs.lock().get(job_id).cloned()
    }
}

#[test]
fn job_messages_returns_in_memory_messages_when_present() {
    let registry = SubagentJobRegistry::new();
    let id = registry.register(SubagentJobInit {
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

#[test]
fn session_transcript_store_is_selected_by_job_session_id() {
    let registry = SubagentJobRegistry::new();
    let global = Arc::new(MemoryTranscriptStore::default());
    let session_a = Arc::new(MemoryTranscriptStore::default());
    let session_b = Arc::new(MemoryTranscriptStore::default());
    registry.set_transcript_store(Some(global.clone()));
    registry.set_session_transcript_store(Some("session-a".into()), session_a.clone());
    registry.set_session_transcript_store(Some("session-b".into()), session_b.clone());

    let id_a = registry.register(SubagentJobInit {
        agent: "a".into(),
        source: "dag".into(),
        run_id: Some("run-a".into()),
        node_id: Some("a".into()),
        session_id: Some("session-a".into()),
    });
    registry.update(&id_a, |job| {
        job.messages.push(serde_json::json!({"text": "a"}));
    });
    registry.finish(&id_a, SubagentJobStatus::Succeeded, None);

    let id_b = registry.register(SubagentJobInit {
        agent: "b".into(),
        source: "dag".into(),
        run_id: Some("run-b".into()),
        node_id: Some("b".into()),
        session_id: Some("session-b".into()),
    });
    registry.update(&id_b, |job| {
        job.messages.push(serde_json::json!({"text": "b"}));
    });
    registry.finish(&id_b, SubagentJobStatus::Succeeded, None);

    assert!(global.nodes.lock().is_empty());
    assert_eq!(
        session_a.load_node("run-a", "a"),
        Some(vec![serde_json::json!({"text": "a"})])
    );
    assert!(session_a.load_node("run-b", "b").is_none());
    assert_eq!(
        session_b.load_node("run-b", "b"),
        Some(vec![serde_json::json!({"text": "b"})])
    );
    assert!(session_b.load_node("run-a", "a").is_none());

    for (session, text) in [("session-a", "memory-a"), ("session-b", "memory-b")] {
        let id = registry.register(SubagentJobInit {
            agent: session.into(),
            source: "dag".into(),
            run_id: Some("run-shared".into()),
            node_id: Some("node".into()),
            session_id: Some(session.into()),
        });
        registry.update(&id, |job| {
            job.messages.push(serde_json::json!({"text": text}));
        });
    }
    assert_eq!(
        registry.node_messages_for_session(Some("session-a"), "run-shared", "node"),
        Some(vec![serde_json::json!({"text": "memory-a"})])
    );
    assert_eq!(
        registry.node_messages_for_session(Some("session-b"), "run-shared", "node"),
        Some(vec![serde_json::json!({"text": "memory-b"})])
    );

    // Stored session transcripts must not leak through another session or the
    // global compatibility lookup.
    session_a.save(&JobTranscript {
        job_id: "stored",
        run_id: Some("run-stored"),
        node_id: Some("a"),
        messages: &[serde_json::json!({"text": "stored"})],
    });
    assert_eq!(
        registry.node_messages_for_session(Some("session-a"), "run-stored", "a"),
        Some(vec![serde_json::json!({"text": "stored"})])
    );
    assert!(
        registry
            .node_messages_for_session(Some("session-b"), "run-stored", "a")
            .is_none()
    );
    assert!(registry.node_messages("run-stored", "a").is_none());

    global.save(&JobTranscript {
        job_id: "global",
        run_id: Some("run-global"),
        node_id: Some("g"),
        messages: &[serde_json::json!({"text": "global"})],
    });
    assert_eq!(
        registry.node_messages_for_session(Some("unregistered"), "run-global", "g"),
        Some(vec![serde_json::json!({"text": "global"})])
    );
}
