use tempfile::TempDir;
use theway_core::multiagent::jobs::{SubagentJobInit, SubagentJobStatus, append_message};

use super::*;
use crate::orchestration::SessionRuntimeBuilder;
use crate::test_env::{ENV_LOCK, EnvGuard};

fn finish_shared_node(
    factory: &SessionRuntimeBuilder,
    session_id: &str,
    text: &str,
) {
    let job_id = factory.subagent_registry.register(SubagentJobInit {
        agent: "tester".into(),
        source: "dag".into(),
        run_id: Some("shared-run".into()),
        node_id: Some("shared-node".into()),
        session_id: Some(session_id.to_string()),
    });
    factory.subagent_registry.update(&job_id, |job| {
        append_message(job, &serde_json::json!({ "text": text }));
    });
    factory
        .subagent_registry
        .finish(&job_id, SubagentJobStatus::Succeeded, None);
}

#[tokio::test]
async fn builds_register_cwd_owned_transcript_stores_and_persist_shared_node_isolated() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_a = TempDir::new().unwrap();
    let work_b = TempDir::new().unwrap();
    let base_a = TempDir::new().unwrap();
    let base_b = TempDir::new().unwrap();
    let repo_root_a = TempDir::new().unwrap();
    let repo_root_b = TempDir::new().unwrap();
    let repo_a = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_a.path());
    let repo_b = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_b.path());
    let id_a = create_session_with_cwd(&repo_a, work_a.path().to_str().unwrap()).await;
    let id_b = create_session_with_cwd(&repo_b, work_b.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let ctx_a = session_context(work_a.path(), repo_a, storage.clone(), base_a.path()).await;
    let ctx_b = session_context(work_b.path(), repo_b, storage, base_b.path()).await;

    let runtime_a = factory
        .build(&ctx_a, &id_a)
        .await
        .expect("cwd A runtime builds");
    let runtime_b = factory
        .build(&ctx_b, &id_b)
        .await
        .expect("cwd B runtime builds");

    finish_shared_node(&factory, &id_a, "alpha");
    finish_shared_node(&factory, &id_b, "beta");

    let path_a = runtime_a
        .cwd
        .join(".pi/subagent-jobs/shared-run/shared-node.json");
    let path_b = runtime_b
        .cwd
        .join(".pi/subagent-jobs/shared-run/shared-node.json");
    let raw_a = std::fs::read_to_string(&path_a).expect("session A transcript file");
    let raw_b = std::fs::read_to_string(&path_b).expect("session B transcript file");
    let messages_a: Vec<serde_json::Value> = serde_json::from_str(&raw_a).unwrap();
    let messages_b: Vec<serde_json::Value> = serde_json::from_str(&raw_b).unwrap();

    assert_eq!(messages_a.len(), 1);
    assert_eq!(messages_a[0]["text"], "alpha");
    assert_eq!(messages_b.len(), 1);
    assert_eq!(messages_b[0]["text"], "beta");
    assert!(!raw_a.contains("beta"), "A transcript leaked B: {raw_a}");
    assert!(!raw_b.contains("alpha"), "B transcript leaked A: {raw_b}");
}
