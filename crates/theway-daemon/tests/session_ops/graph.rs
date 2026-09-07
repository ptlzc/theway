use super::*;
use super::super::graph::{
    get_session_graph_node, list_session_graph_node_messages, list_session_graph_nodes,
    session_lineage,
};
use theway_storage::session_graph::SessionGraphNode;

fn node(id: &str, source_session_id: Option<&str>) -> SessionGraphNode {
    SessionGraphNode {
        id: id.to_string(),
        node_type: "collapsed".to_string(),
        parent_id: None,
        child_ids: Vec::new(),
        name: format!("Node {id}"),
        status: "collapsed".to_string(),
        summary: Some("summary".to_string()),
        raw_text_ref: source_session_id.map(str::to_string),
        source_session_id: source_session_id.map(str::to_string),
        run_id: None,
        node_id: None,
        job_id: None,
        subagent_graph: serde_json::Value::Null,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: Some("2026-01-01T00:00:00Z".to_string()),
    }
}

#[tokio::test]
async fn get_session_graph_node_requires_configured_store() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let ops = ops(repo, "current");
    let err = get_session_graph_node(&ops, "session-1", "node-1")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session graph store not configured"));
}

#[tokio::test]
async fn graph_node_query_round_trips_through_store() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let graph_path = dir.path().join("session_graph.db");
    let session_id = "session-1";
    let ops = AppSessionOps::with_session_graph(
        repo,
        Arc::new(DagEngine::new()),
        "/cwd".into(),
        SessionExecutionRegistry::new(),
        SubagentJobRegistry::new(),
        graph_path.clone(),
    );

    assert!(get_session_graph_node(&ops, session_id, "missing")
        .await
        .unwrap()
        .is_none());

    let store = SessionGraphStore::open(&graph_path).await.unwrap();
    store.save_node(node("node-1", Some(session_id))).await.unwrap();
    store.save_node(node("node-2", None)).await.unwrap();

    let loaded = get_session_graph_node(&ops, session_id, "node-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.id, "node-1");
    assert_eq!(loaded.session_id, session_id);
    assert_eq!(loaded.collapsed_session_id.as_deref(), Some(session_id));
    assert_eq!(loaded.title, "Node node-1");

    let listed = list_session_graph_nodes(&ops, session_id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|n| n.id == "node-1"));
    assert!(listed.iter().any(|n| n.id == "node-2"));
}

#[tokio::test]
async fn session_lineage_reads_collapsed_metadata_flag() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let id = session_id_of(&session).await;
    let ops = ops(repo.clone(), &id);

    assert!(session_lineage(&ops, &id).await.unwrap().collapsed_from_session_id.is_none());

    session.set_collapsed(true).await.unwrap();
    let lineage = session_lineage(&ops, &id).await.unwrap();
    assert_eq!(lineage.collapsed_from_session_id.as_deref(), Some(id.as_str()));
}

#[tokio::test]
async fn list_session_graph_node_messages_filters_and_limits() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let graph_path = dir.path().join("session_graph.db");
    let source = repo.create("/cwd").await.unwrap();
    let source_id = session_id_of(&source).await;
    let graph_ops = AppSessionOps::with_session_graph(
        repo.clone(),
        Arc::new(DagEngine::new()),
        "/cwd".into(),
        SessionExecutionRegistry::new(),
        SubagentJobRegistry::new(),
        graph_path.clone(),
    );

    source
        .append_entry(
            StoredSessionEntry::from_payload(serde_json::json!({
                "type": "message",
                "id": "msg-1",
                "parentId": null,
                "timestamp": "2026-08-24T00:00:00Z",
                "message": { "role": "user", "content": "hello" },
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    source
        .append_entry(
            StoredSessionEntry::from_payload(serde_json::json!({
                "type": "custom",
                "id": "custom-1",
                "parentId": null,
                "timestamp": "2026-08-24T00:00:00Z",
                "customType": "note",
                "data": { "text": "ignored" },
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    source
        .append_entry(
            StoredSessionEntry::from_payload(serde_json::json!({
                "type": "message",
                "id": "msg-2",
                "parentId": null,
                "timestamp": "2026-08-24T00:00:00Z",
                "message": { "role": "tool", "content": { "tool_call_id": "t1" } },
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    // Without a graph path, messages are read from the supplied session.
    let no_graph = ops(repo.clone(), &source_id);
    let messages = list_session_graph_node_messages(&no_graph, &source_id, "node-x", 0, 0)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2, "custom entries are filtered out");

    // A graph node with a source session id redirects the message query.
    let store = SessionGraphStore::open(&graph_path).await.unwrap();
    store
        .save_node(node("node-redir", Some(&source_id)))
        .await
        .unwrap();
    let messages = list_session_graph_node_messages(&graph_ops, &source_id, "node-redir", 0, 1)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);

    // A graph node without a source id falls back to the requested session.
    store.save_node(node("node-fallback", None)).await.unwrap();
    let messages = list_session_graph_node_messages(&graph_ops, &source_id, "node-fallback", 1, 0)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
}
