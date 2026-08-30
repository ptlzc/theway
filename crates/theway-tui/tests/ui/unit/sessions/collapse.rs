/// Issue #56: `/collapse` is part of the daemon-side command surface the
/// TUI exposes in completion (the daemon owns the interactive collapse
/// workflow and session switch).
#[test]
fn collect_slash_commands_includes_daemon_collapse_command() {
    let registry = crate::local_commands::local_registry();
    let commands = collect_slash_commands(&registry, &[], &[], &[]);

    assert!(
        commands.contains(&"/collapse".to_string()),
        "completion list must contain /collapse, got: {commands:?}"
    );
    assert!(
        super::DAEMON_COMMANDS.contains(&"collapse"),
        "/collapse must live in the daemon command table"
    );
}

/// session-snapshot-collapse: the side panel renders session lineage and
/// collapsed graph nodes when a nested snapshot has been fetched.
#[tokio::test]
async fn side_panel_renders_session_lineage_and_collapsed_nodes() {
    use theway_transport::wire::{
        WireModelRef, WireSessionFeed, WireSessionGraphNode, WireSessionGraphNodeType,
        WireSessionGraphState, WireSessionInfo, WireSessionLineage, WireSessionRuntime,
        WireSessionSnapshot,
    };

    let (mut app, _rx) = test_app().await;
    let node = WireSessionGraphNode {
        id: "node-collapsed".into(),
        session_id: "sess-new".into(),
        node_type: WireSessionGraphNodeType::Collapsed,
        title: "Archived".into(),
        summary: "Old work".into(),
        parent_node_id: None,
        child_node_ids: Vec::new(),
        collapsed_session_id: Some("sess-old".into()),
        collapsed_at: Some("2026-08-01T00:00:00Z".into()),
        created_at: None,
        updated_at: None,
        message_count: 7,
    };
    app.session_snapshot = Some(WireSessionSnapshot {
        session_id: "sess-new".into(),
        info: WireSessionInfo {
            id: "sess-new".into(),
            name: "child".into(),
            cwd: "/tmp/theway".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            last_activity_at: 0,
            last_activity_at_rfc3339: None,
            busy: false,
            preview: None,
            metadata: Default::default(),
            graph_count: 0,
            active_graph_count: 0,
            queued_count: 0,
            sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        },
        runtime: WireSessionRuntime {
            model: WireModelRef {
                provider: "provider".into(),
                model: "model".into(),
                base_url: None,
            },
            thinking_level: "high".into(),
            supported_thinking_levels: vec![],
            context_usage: Default::default(),
            session_context_usage: Default::default(),
            tui_max_feed_lines: None,
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: Default::default(),
            system_context: String::new(),
        },
        feed: WireSessionFeed {
            blocks: Vec::new(),
            lines: Vec::new(),
            blocks_base: 0,
            lines_base: 0,
            block_patches: Vec::new(),
        },
        graph_state: WireSessionGraphState {
            dags: Vec::new(),
            subagents: Vec::new(),
            nodes: vec![node],
            active_node_id: Some("node-collapsed".into()),
        },
        lineage: WireSessionLineage {
            parent_session_id: Some("sess-old".into()),
            root_session_id: Some("sess-old".into()),
            ancestor_session_ids: vec!["sess-old".into()],
            child_session_ids: Vec::new(),
            collapsed_from_session_id: None,
            collapsed_into_session_id: Some("sess-new".into()),
        },
    });
    app.side_panel_mode = super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH);

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());

    assert!(text.contains("Session"), "session section missing:\n{text}");
    assert!(
        text.contains("collapsed into sess-new"),
        "lineage missing:\n{text}"
    );
    assert!(
        text.contains("node-collapsed"),
        "collapsed node id missing:\n{text}"
    );
    assert!(
        text.contains("Archive"),
        "collapsed node title missing:\n{text}"
    );
}
