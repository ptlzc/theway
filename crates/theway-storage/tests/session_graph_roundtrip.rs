//! End-to-end tests for the Turso-backed session graph store.

use std::path::PathBuf;

use serde_json::json;
use theway_storage::session_graph::{SessionGraphNode, SessionGraphStore};

fn sample_node(id: &str, parent_id: Option<&str>) -> SessionGraphNode {
    SessionGraphNode {
        id: id.to_string(),
        node_type: "session".to_string(),
        parent_id: parent_id.map(str::to_string),
        name: format!("node {id}"),
        status: "collapsed".to_string(),
        summary: Some("compact summary".to_string()),
        raw_text_ref: Some(format!("{id}#raw")),
        source_session_id: Some("source-session".to_string()),
        run_id: None,
        node_id: None,
        job_id: None,
        subagent_graph: json!({
            "dags": [],
            "subagents": []
        }),
        child_ids: Vec::new(),
        created_at: "2026-08-24T00:00:00Z".to_string(),
        updated_at: Some("2026-08-24T00:01:00Z".to_string()),
    }
}

fn temp_db(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("session-graph-{name}-{}.db", std::process::id()))
}

async fn open_clean(name: &str) -> SessionGraphStore {
    let path = temp_db(name);
    let _ = std::fs::remove_file(&path);
    SessionGraphStore::open(&path).await.unwrap()
}

#[test]
fn session_graph_node_serializes_with_wire_names() {
    let node = sample_node("wire", None);
    let json = serde_json::to_value(&node).unwrap();
    assert_eq!(json["type"], "session");
    assert_eq!(json["childIds"], json!([]));
    assert_eq!(json["subagentGraph"]["dags"], json!([]));
    assert_eq!(json["sourceSessionId"], "source-session");
}

#[tokio::test]
async fn save_and_load_node_round_trip() {
    let store = open_clean("roundtrip").await;
    let mut parent = sample_node("parent", None);
    let mut child = sample_node("child", Some("parent"));
    child.child_ids = vec!["grandchild".to_string()];
    parent.child_ids = vec!["child".to_string()];

    store.save_node(&parent).await.unwrap();
    store.save_node(&child).await.unwrap();

    let loaded = store.load_node("child").await.unwrap().unwrap();
    assert_eq!(loaded.id, "child");
    assert_eq!(loaded.parent_id.as_deref(), Some("parent"));
    assert_eq!(loaded.child_ids, vec!["grandchild"]);
    assert_eq!(loaded.subagent_graph["dags"], json!([]));
    assert_eq!(loaded.summary.as_deref(), Some("compact summary"));

    let nodes = store.list_nodes().await.unwrap();
    assert_eq!(nodes.len(), 2);

    let edges = store.list_edges().await.unwrap();
    assert_eq!(edges.len(), 2);
    assert!(
        edges.contains(&theway_storage::session_graph::SessionGraphEdge {
            parent_id: "parent".to_string(),
            child_id: "child".to_string(),
        })
    );
    assert!(
        edges.contains(&theway_storage::session_graph::SessionGraphEdge {
            parent_id: "child".to_string(),
            child_id: "grandchild".to_string(),
        })
    );

    let _ = std::fs::remove_file(temp_db("roundtrip"));
}

#[tokio::test]
async fn save_node_replaces_edges_for_that_parent() {
    let store = open_clean("edges").await;
    let mut node = sample_node("a", None);
    node.child_ids = vec!["old-1".to_string(), "old-2".to_string()];
    store.save_node(&node).await.unwrap();

    node.child_ids = vec!["new-1".to_string()];
    store.save_node(&node).await.unwrap();

    let edges = store.list_edges().await.unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].parent_id, "a");
    assert_eq!(edges[0].child_id, "new-1");

    let _ = std::fs::remove_file(temp_db("edges"));
}

#[tokio::test]
async fn nodes_persist_across_reopen() {
    let path = temp_db("reopen");
    let _ = std::fs::remove_file(&path);
    {
        let store = SessionGraphStore::open(&path).await.unwrap();
        store
            .save_node(&sample_node("persisted", None))
            .await
            .unwrap();
    }
    let reopened = SessionGraphStore::open(&path).await.unwrap();
    let node = reopened.load_node("persisted").await.unwrap().unwrap();
    assert_eq!(node.id, "persisted");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn save_node_links_nested_chain_and_survives_reopen() {
    let path = temp_db("nested-chain");
    let _ = std::fs::remove_file(&path);
    {
        let store = SessionGraphStore::open(&path).await.unwrap();
        store.save_node(&sample_node("node-1", None)).await.unwrap();
        store
            .save_node(&sample_node("node-2", Some("node-1")))
            .await
            .unwrap();
        store
            .save_node(&sample_node("node-3", Some("node-2")))
            .await
            .unwrap();
    }

    let reopened = SessionGraphStore::open(&path).await.unwrap();
    let node1 = reopened.load_node("node-1").await.unwrap().unwrap();
    let node2 = reopened.load_node("node-2").await.unwrap().unwrap();
    let node3 = reopened.load_node("node-3").await.unwrap().unwrap();

    assert_eq!(node2.parent_id.as_deref(), Some("node-1"));
    assert_eq!(node3.parent_id.as_deref(), Some("node-2"));
    assert_eq!(node1.child_ids, vec!["node-2"]);
    assert_eq!(node2.child_ids, vec!["node-3"]);

    let edges = reopened.list_edges().await.unwrap();
    assert!(
        edges.contains(&theway_storage::session_graph::SessionGraphEdge {
            parent_id: "node-1".to_string(),
            child_id: "node-2".to_string(),
        })
    );
    assert!(
        edges.contains(&theway_storage::session_graph::SessionGraphEdge {
            parent_id: "node-2".to_string(),
            child_id: "node-3".to_string(),
        })
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn link_child_updates_parent_child_ids_and_edge_atomically() {
    let store = open_clean("link-child").await;
    store.save_node(&sample_node("parent", None)).await.unwrap();

    store.link_child("parent", "child").await.unwrap();

    let parent = store.load_node("parent").await.unwrap().unwrap();
    assert_eq!(parent.child_ids, vec!["child"]);
    assert!(parent.updated_at.is_some());
    let edges = store.list_edges().await.unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].parent_id, "parent");
    assert_eq!(edges[0].child_id, "child");

    // Idempotent: linking twice must not duplicate the child id or the edge.
    store.link_child("parent", "child").await.unwrap();
    let parent = store.load_node("parent").await.unwrap().unwrap();
    assert_eq!(parent.child_ids, vec!["child"]);
    assert_eq!(store.list_edges().await.unwrap().len(), 1);

    let _ = std::fs::remove_file(temp_db("link-child"));
}

#[tokio::test]
async fn missing_node_returns_none() {
    let store = open_clean("missing").await;
    assert!(store.load_node("nope").await.unwrap().is_none());
    let _ = std::fs::remove_file(temp_db("missing"));
}
