use super::*;

#[tokio::test]
async fn collapse_creates_child_and_registers_graph_node() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let source = repo.create("/cwd").await.unwrap();
    let source_id = session_id_of(&source).await;
    source
        .append_entry(
            StoredSessionEntry::from_payload(serde_json::json!({
                "type": "custom",
                "id": "compact-1",
                "parentId": null,
                "timestamp": "2026-08-24T00:00:00Z",
                "customType": "compact_context",
                "data": { "text": "old" },
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let graph_path = dir.path().join("session_graph.db");
    let ops = AppSessionOps::with_session_graph(
        repo.clone(),
        Arc::new(DagEngine::new()),
        "/cwd".into(),
        SessionExecutionRegistry::new(),
        SubagentJobRegistry::new(),
        graph_path.clone(),
    );
    let request = WireCollapseSessionRequest {
        session_id: source_id.clone(),
        into_session_id: None,
        title: Some("Archived".into()),
        summary: Some("compact summary".into()),
    };
    let response = ops.collapse_session(&request).await.unwrap();
    let child_id = response
        .collapsed
        .as_ref()
        .unwrap()
        .collapsed_into_session_id
        .clone()
        .unwrap();
    let node_id = response.node.as_ref().unwrap().id.clone();
    assert_ne!(child_id, source_id);
    assert!(response.node.as_ref().unwrap().title.contains("Archived"));

    let store = SessionGraphStore::open(&graph_path).await.unwrap();
    let node = store.load_node(&node_id).await.unwrap().unwrap();
    assert_eq!(node.node_type, "collapsed");
    assert_eq!(node.source_session_id.as_deref(), Some(source_id.as_str()));

    let child = SessionRepository::open(repo.as_ref(), &child_id)
        .await
        .unwrap()
        .unwrap();
    let entries = child.get_entries().await.unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.payload["customType"] == "compact_context"),
        "child must carry compact context entries"
    );

    let session = theway_core::Session::from_store(child);
    let ctx = session.build_context().await.unwrap();
    let provider_messages = theway_core::default_convert_to_llm()(&ctx.messages);
    assert!(
        provider_messages.iter().any(|m| {
            if let theway_llm_provider::Message::User(u) = m {
                if let theway_llm_provider::UserContent::Text(text) = &u.content {
                    return text.contains("compact summary");
                }
            }
            false
        }),
        "collapse child build_context should materialize compact summary"
    );
}

#[tokio::test]
async fn nested_collapse_links_node_chain_and_rolls_summary_forward() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let source = repo.create("/cwd").await.unwrap();
    let source_id = session_id_of(&source).await;
    theway_core::Session::from_store(Arc::new(source))
        .append_compaction(
            concat!(
                "goal: gen-1 goal\n",
                "completed work: gen-1 completed\n",
                "key decisions: gen-1 decision\n",
                "next steps: gen-1 next\n",
                "critical context: gen-1 critical",
            ),
            "first",
            10,
            None,
            true,
        )
        .await
        .unwrap();

    let graph_path = dir.path().join("session_graph.db");
    let ops = AppSessionOps::with_session_graph(
        repo.clone(),
        Arc::new(DagEngine::new()),
        "/cwd".into(),
        SessionExecutionRegistry::new(),
        SubagentJobRegistry::new(),
        graph_path.clone(),
    );

    // S -> C1
    let first = ops
        .collapse_session(&WireCollapseSessionRequest {
            session_id: source_id.clone(),
            into_session_id: None,
            title: Some("First collapse".into()),
            summary: None,
        })
        .await
        .unwrap();
    let c1_id = first
        .collapsed
        .as_ref()
        .unwrap()
        .collapsed_into_session_id
        .clone()
        .unwrap();
    let node1_id = first.node.as_ref().unwrap().id.clone();
    assert_eq!(first.node.as_ref().unwrap().parent_node_id, None);

    // C1 -> C2: the source is a collapse child, so the parent node id is
    // C1's collapseNodeId and the previous compact summary rolls forward.
    let second = ops
        .collapse_session(&WireCollapseSessionRequest {
            session_id: c1_id.clone(),
            into_session_id: None,
            title: Some("Second collapse".into()),
            summary: None,
        })
        .await
        .unwrap();
    let c2_id = second
        .collapsed
        .as_ref()
        .unwrap()
        .collapsed_into_session_id
        .clone()
        .unwrap();
    let node2_id = second.node.as_ref().unwrap().id.clone();
    assert_eq!(
        second.node.as_ref().unwrap().parent_node_id.as_deref(),
        Some(node1_id.as_str())
    );

    // Node chain persists: node1.child_ids == [node2], reopen still reads it.
    let store = SessionGraphStore::open(&graph_path).await.unwrap();
    let node1 = store.load_node(&node1_id).await.unwrap().unwrap();
    let node2 = store.load_node(&node2_id).await.unwrap().unwrap();
    assert_eq!(node2.parent_id.as_deref(), Some(node1_id.as_str()));
    assert_eq!(node1.child_ids, vec![node2_id.clone()]);
    let nodes = store.list_nodes().await.unwrap();
    assert_eq!(nodes.len(), 2);

    // C2's compact_context is a bounded five-component rolling summary
    // carrying the previous generation's summary text.
    let c2_store = SessionRepository::open(repo.as_ref(), &c2_id)
        .await
        .unwrap()
        .unwrap();
    let compact = theway_core::Session::from_store(c2_store)
        .compact_context()
        .await
        .unwrap()
        .expect("compact context");
    assert_eq!(compact.source_session_id, c1_id);
    for component in [
        "goal",
        "completed work",
        "key decisions",
        "next steps",
        "critical context",
    ] {
        assert!(
            compact.compact_text.contains(&format!("{component}: ")),
            "C2 rolling summary must carry {component:?}"
        );
    }
    assert!(compact.compact_text.contains("gen-1 critical"));
    assert!(compact.compact_text.contains("gen-1 completed"));
    for line in compact.compact_text.lines() {
        let (name, value) = line.split_once(": ").unwrap();
        let limit = ROLLING_SUMMARY_COMPONENTS
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap()
            .1;
        assert!(value.chars().count() <= limit);
    }

    // Event-only lineage: ids present, summary text absent.
    let lineage = crate::context::lineage::render_lineage(Some(&compact), Some(&node2_id))
        .expect("lineage");
    assert!(lineage.contains(&format!("node id: {node2_id}")));
    assert!(lineage.contains(&format!("source session id: {c1_id}")));
    assert!(!lineage.contains("gen-1"));
    assert!(!lineage.contains("compactText"));
}

#[tokio::test]
async fn collapse_into_existing_session_links_compact_context_to_active_branch() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let source = repo.create("/cwd").await.unwrap();
    let source_id = session_id_of(&source).await;
    let target = repo.create("/cwd").await.unwrap();
    let target_id = session_id_of(&target).await;
    target
        .append_entry(
            StoredSessionEntry::from_payload(serde_json::json!({
                "type": "message",
                "id": "msg-1",
                "parentId": null,
                "timestamp": "2026-08-24T00:00:00Z",
                "message": { "role": "user", "content": "existing", "timestamp": 1 },
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let graph_path = dir.path().join("session_graph.db");
    let ops = AppSessionOps::with_session_graph(
        repo.clone(),
        Arc::new(DagEngine::new()),
        "/cwd".into(),
        SessionExecutionRegistry::new(),
        SubagentJobRegistry::new(),
        graph_path.clone(),
    );
    let request = WireCollapseSessionRequest {
        session_id: source_id.clone(),
        into_session_id: Some(target_id.clone()),
        title: Some("Archived".into()),
        summary: Some("compact summary".into()),
    };
    let response = ops.collapse_session(&request).await.unwrap();
    assert!(response.node.is_some());

    let target_store = SessionRepository::open(repo.as_ref(), &target_id)
        .await
        .unwrap()
        .unwrap();
    let entries = target_store.get_entries().await.unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.payload["customType"] == "compact_context"),
        "target must carry compact context entries"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.payload["customType"] == "session_graph_state"),
        "target must carry session graph state entry"
    );

    let leaf = target_store.get_leaf_id().await.unwrap().unwrap();
    let path = target_store.get_path_to_root(Some(&leaf)).await.unwrap();
    assert!(
        path.iter()
            .any(|e| e.payload["customType"] == "compact_context"),
        "compact context must be on the active branch"
    );

    let session = theway_core::Session::from_store(target_store);
    let ctx = session.build_context().await.unwrap();
    let provider_messages = theway_core::default_convert_to_llm()(&ctx.messages);
    assert!(
        provider_messages.iter().any(|m| {
            if let theway_llm_provider::Message::User(u) = m {
                if let theway_llm_provider::UserContent::Text(text) = &u.content {
                    return text.contains("compact summary");
                }
            }
            false
        }),
        "build_context should materialize the compact summary message"
    );
}
