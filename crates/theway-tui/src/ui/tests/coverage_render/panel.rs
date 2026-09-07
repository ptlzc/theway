use super::*;

#[tokio::test]
async fn panel_should_show_all_contributors_and_empty_state() {
    let (mut app, _rx) = test_app().await;
    assert!(!app.should_show_side_panel(), "empty fixture must hide");

    let mut status = fixture_status(Vec::new());
    let mut sidebar = theway_transport::testing::empty_sidebar_snapshot();
    sidebar.skills.items = vec![WireSkillSnapshot {
        name: "code-review".into(),
        source: "user".into(),
        file_path: "/skills/code-review".into(),
        enabled: true,
    }];
    sidebar.triggers.rules = vec![WireTriggerRuleSnapshot {
        id: "rule-1".into(),
        full_id: "rule-1".into(),
        enabled: true,
        mode: "dynamic".into(),
        condition: "x".into(),
        action: "y".into(),
    }];
    sidebar.cron.jobs = vec![WireCronJobSnapshot {
        id: "cron-1".into(),
        enabled: true,
        schedule: "* * * * *".into(),
        action: "run".into(),
        skipped_overlap_count: 1,
        last_error: None,
    }];
    sidebar.mcp.servers = 1;
    sidebar.mcp.notification_hooks = 1;
    sidebar.mcp.errors = vec![theway_transport::wire::WireMcpServerError {
        name: "mcp".into(),
        error: "boom".into(),
    }];
    status.sidebar = sidebar;
    status.latest_trigger_poll = Some(TriggerPollStatus {
        checked_at: "now".into(),
        trace_id: "trace".into(),
        source_label: "file".into(),
        event_label: "write".into(),
        summary: "summary".into(),
    });
    status.sidebar.inbox_new = 2;
    status.goal = Some(WireGoalSnapshot {
        condition: "done".into(),
        status: "pursuing".into(),
        iterations: 2,
        last_reason: Some("because".into()),
    });
    status.extensions.catalog = vec![WireExtensionCatalogEntry {
        extension_id: "ext".into(),
        version: "1.0".into(),
        source: "user".into(),
        scope: "project".into(),
        priority: 0,
        status: "effective".into(),
        permissions: vec![],
        reason_code: None,
    }];
    status.extensions.reload_pending = true;
    status.extensions.contributions = vec![
        WireExtensionContribution {
            contribution_id: "c1".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "status_item".into(),
            payload: serde_json::json!({"label": "cpu", "value": "12%"}),
        },
        WireExtensionContribution {
            contribution_id: "c2".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "notification".into(),
            payload: serde_json::json!({"title": "hello"}),
        },
        WireExtensionContribution {
            contribution_id: "c3".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "unknown".into(),
            payload: serde_json::json!({}),
        },
    ];
    status.dags = vec![dag_run("dag-1", "running")];
    app.apply_snapshot(status);
    app.graph_position = crate::ui::GraphPosition::SidePanel;
    app.session_snapshot = Some(empty_session_snapshot());
    assert!(app.should_show_side_panel());

    let lines = app.trigger_panel_lines(80, 1000);
    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();
    assert!(text.contains("Extensions"), "{text}");
    assert!(
        text.contains("active 1 · unavailable 0 · reload pending"),
        "{text}"
    );
    assert!(text.contains("cpu: 12%"), "{text}");
    assert!(text.contains("Session"), "{text}");
    assert!(text.contains("Skills"), "{text}");
    assert!(text.contains("Graph"), "{text}");
    assert!(text.contains("Triggers"), "{text}");
    assert!(text.contains("Polling"), "{text}");
    assert!(text.contains("Goal"), "{text}");
    assert!(text.contains("Inbox"), "{text}");
    assert!(text.contains("Cron (session)"), "{text}");
    assert!(text.contains("MCP"), "{text}");
    // Hooks/Runtime are diagnostic-only and hidden by default.
    assert!(!text.contains("Hooks"), "{text}");
    assert!(!text.contains("Runtime"), "{text}");
    app.show_hooks = true;
    app.show_runtime = true;
    let lines = app.trigger_panel_lines(80, 1000);
    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();
    assert!(text.contains("Hooks"), "{text}");
    assert!(text.contains("Runtime"), "{text}");
}

#[tokio::test]
async fn panel_lines_covers_goal_statuses_and_session_graph_arms() {
    let (mut app, _rx) = test_app().await;
    for status in [
        "pursuing",
        "achieved",
        "paused",
        "budget_limited",
        "cleared",
        "unknown",
    ] {
        app.latest.goal = Some(WireGoalSnapshot {
            condition: "c".into(),
            status: status.into(),
            iterations: 0,
            last_reason: None,
        });
        let text = app
            .trigger_panel_lines(40, 200)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();
        assert!(text.contains(status), "{status}: {text}");
    }

    let mut snapshot = empty_session_snapshot();
    snapshot.lineage.parent_session_id = Some("parent".into());
    snapshot.lineage.root_session_id = Some("root".into());
    snapshot.lineage.ancestor_session_ids = vec!["a".into()];
    snapshot.lineage.child_session_ids = vec!["child".into()];
    snapshot.lineage.collapsed_from_session_id = Some("from".into());
    snapshot.lineage.collapsed_into_session_id = Some("into".into());
    snapshot.graph_state.nodes = vec![
        WireSessionGraphNode {
            id: "n1".into(),
            session_id: "s".into(),
            node_type: WireSessionGraphNodeType::Collapsed,
            title: "Collapsed".into(),
            summary: "sum".into(),
            parent_node_id: None,
            child_node_ids: vec![],
            collapsed_session_id: None,
            collapsed_at: None,
            created_at: None,
            updated_at: None,
            message_count: 3,
        },
        WireSessionGraphNode {
            id: "n2".into(),
            session_id: "s".into(),
            node_type: WireSessionGraphNodeType::Session,
            title: "Session".into(),
            summary: "sum".into(),
            parent_node_id: None,
            child_node_ids: vec![],
            collapsed_session_id: None,
            collapsed_at: None,
            created_at: None,
            updated_at: None,
            message_count: 1,
        },
        WireSessionGraphNode {
            id: "n3".into(),
            session_id: "s".into(),
            node_type: WireSessionGraphNodeType::Unspecified,
            title: "Node".into(),
            summary: "sum".into(),
            parent_node_id: None,
            child_node_ids: vec![],
            collapsed_session_id: None,
            collapsed_at: None,
            created_at: None,
            updated_at: None,
            message_count: 0,
        },
    ];
    snapshot.graph_state.active_node_id = Some("n2".into());
    app.session_snapshot = Some(snapshot);
    let text = app
        .trigger_panel_lines(40, 200)
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();
    assert!(text.contains("parent parent"), "{text}");
    assert!(text.contains("root root"), "{text}");
    assert!(text.contains("ancestors a"), "{text}");
    assert!(text.contains("children child"), "{text}");
    assert!(text.contains("collapsed from from"), "{text}");
    assert!(text.contains("collapsed into into"), "{text}");
    assert!(text.contains("active n2"), "{text}");
    // Session id row is the compact id suffix with no leading ellipsis.
    assert!(text.contains("ess-new · child"), "{text}");
    assert!(!text.contains("…sess-new"), "{text}");
    // Node rows are id-only: no kind/title/msg decoration.
    assert!(text.contains("  n1"), "{text}");
    assert!(text.contains("  n2"), "{text}");
    assert!(text.contains("  n3"), "{text}");
    assert!(!text.contains("collapsed n1"), "{text}");
    assert!(!text.contains("node n3"), "{text}");
    assert!(!text.contains("msgs"), "{text}");
}

#[tokio::test]
async fn panel_lines_covers_skill_cron_trigger_limits_and_status_empty() {
    let (mut app, _rx) = test_app().await;
    app.latest.sidebar.skills.items = (0..3)
        .map(|i| WireSkillSnapshot {
            name: format!("s{i}"),
            source: if i == 0 {
                "builtin".into()
            } else if i == 1 {
                "user".into()
            } else {
                "project".into()
            },
            file_path: format!("/s{i}"),
            enabled: i != 0,
        })
        .collect();
    app.latest.sidebar.triggers.rules = (0..6)
        .map(|i| WireTriggerRuleSnapshot {
            id: format!("r{i}"),
            full_id: format!("r{i}"),
            enabled: i % 2 == 0,
            mode: "m".into(),
            condition: "c".into(),
            action: "a".into(),
        })
        .collect();
    app.latest.sidebar.cron.jobs = (0..6)
        .map(|i| WireCronJobSnapshot {
            id: format!("c{i}"),
            enabled: i % 2 == 0,
            schedule: "s".into(),
            action: "a".into(),
            skipped_overlap_count: i as u64,
            last_error: if i == 0 { Some("err".into()) } else { None },
        })
        .collect();
    app.panel_status.hook_points = vec!["hook".into()];
    app.panel_status.trigger_features = vec!["feature".into()];
    app.panel_status.mcp_servers = 1;
    app.panel_status.mcp_tools = 2;
    app.panel_status.mcp_notification_hooks = 3;
    app.latest.sidebar.mcp.errors = vec![theway_transport::wire::WireMcpServerError {
        name: "bad".into(),
        error: "e".into(),
    }];
    app.latest.sidebar.inbox_new = 2;
    app.show_hooks = true;
    app.show_runtime = true;
    let text = app
        .trigger_panel_lines(60, 400)
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();
    assert!(text.contains("enabled 2 · disabled 1"), "{text}");
    assert!(text.contains("builtin 1 · user 1 · project 1"), "{text}");
    assert!(text.contains("… 1 more"), "{text}");
    assert!(text.contains("Inbox  2 new — /inbox"), "{text}");
    assert!(text.contains("· hook"), "{text}");
    assert!(text.contains("• feature"), "{text}");
}

#[tokio::test]
async fn render_popups_menu_band_and_status_usage_edges() {
    let (mut app, _rx) = test_app().await;
    assert!(app.menu_band_data().is_none(), "no menu by default");

    app.panel_menu = Some(crate::ui::PanelMenuState {
        level: crate::ui::PanelMenuLevel::Position,
        cursor: 0,
    });
    assert!(app.menu_band_data().is_some());

    app.graph_menu = Some(crate::ui::GraphMenuState {
        level: crate::ui::GraphMenuLevel::Root,
        cursor: 0,
        has_graphs: true,
    });
    assert!(app.menu_band_data().is_some());

    app.latest.model_catalog = vec![theway_transport::wire::ProviderGroup {
        provider: "anthropic".into(),
        has_credential: true,
        models: vec![theway_transport::wire::ModelEntry {
            id: "claude-x".into(),
            name: "Claude X".into(),
        }],
    }];
    app.open_model_picker();
    assert!(app.menu_band_data().is_some());

    // Popups and extension overlay, rendered one at a time so each
    // centered popup is visible in the final buffer.
    let prompt = Some(theway_transport::wire::WireControlPlanePromptSnapshot {
        tool_name: "write".into(),
        label: "write file".into(),
        reason: "need approval".into(),
        args_hash: "abcdef1234567890".into(),
        payload: "{}".into(),
    });
    app.fork_picker = Some(crate::ui::ForkPickerState {
        entries: vec![crate::ui::ForkPickerEntry {
            number: 1,
            preview: "first user message".into(),
        }],
        selected: 0,
        scroll: 0,
    });
    app.resume_picker = Some(crate::ui::ResumePickerState {
        entries: vec![crate::ui::ResumePickerEntry {
            id: "sess-1".into(),
            id_short: "sess-1".into(),
            name: "named".into(),
            path: "/tmp/theway".into(),
            last_activity_at_rfc3339: Some("2026-01-01T00:00:00Z".into()),
            busy: true,
            graph_count: 1,
            active_graph_count: 1,
            current: true,
        }],
        selected: 0,
        scroll: 0,
    });
    app.latest.extensions.catalog = vec![WireExtensionCatalogEntry {
        extension_id: "ext".into(),
        version: "1".into(),
        source: "project".into(),
        scope: "project".into(),
        priority: 0,
        status: "faulted".into(),
        permissions: vec![],
        reason_code: Some("load".into()),
    }];
    app.latest.extensions.diagnostics = vec![theway_transport::wire::WireExtensionDiagnostic {
        extension_id: "ext".into(),
        code: "E1".into(),
        severity: "error".into(),
        message: "boom".into(),
        session_id: None,
        event: None,
        sequence: None,
        details: Default::default(),
        redacted_fields: vec!["key".into()],
    }];
    app.latest.extensions.commands = vec![theway_transport::wire::WireExtensionCommandDescriptor {
        extension_id: "ext".into(),
        name: "cmd".into(),
        label: "Command".into(),
        description: "desc".into(),
        argument_schema: serde_json::json!({}),
    }];
    app.latest.extensions.contributions = vec![
        WireExtensionContribution {
            contribution_id: "c1".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "status_item".into(),
            payload: serde_json::json!({"label": "l", "value": "v"}),
        },
        WireExtensionContribution {
            contribution_id: "c2".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "notification".into(),
            payload: serde_json::json!({"title": "t", "body": "b"}),
        },
        WireExtensionContribution {
            contribution_id: "c3".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "command".into(),
            payload: serde_json::json!({"command": {"name": "cmd"}}),
        },
        WireExtensionContribution {
            contribution_id: "c4".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "detail_panel".into(),
            payload: serde_json::json!({"title": "detail"}),
        },
        WireExtensionContribution {
            contribution_id: "c5".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "form_action".into(),
            payload: serde_json::json!({"title": "action"}),
        },
        WireExtensionContribution {
            contribution_id: "c6".into(),
            extension_id: "ext".into(),
            scope: "project".into(),
            kind: "unknown".into(),
            payload: serde_json::json!({}),
        },
    ];

    app.latest.usage.context_window = 1000;
    app.latest.usage.total_input_tokens = 500;
    app.latest.usage.output_tokens = 100;
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    app.control_plane_prompt = prompt.clone();
    app.fork_picker = None;
    app.resume_picker = None;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("Control-plane approval required"), "{text}");
    assert!(text.contains("provider:model"), "{text}");

    app.control_plane_prompt = None;
    app.resume_picker = None;
    app.fork_picker = Some(crate::ui::ForkPickerState {
        entries: vec![crate::ui::ForkPickerEntry {
            number: 1,
            preview: "first user message".into(),
        }],
        selected: 0,
        scroll: 0,
    });
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("fork"), "{text}");

    app.fork_picker = None;
    app.resume_picker = Some(crate::ui::ResumePickerState {
        entries: vec![crate::ui::ResumePickerEntry {
            id: "sess-1".into(),
            id_short: "sess-1".into(),
            name: "named".into(),
            path: "/tmp/theway".into(),
            last_activity_at_rfc3339: Some("2026-01-01T00:00:00Z".into()),
            busy: true,
            graph_count: 1,
            active_graph_count: 1,
            current: true,
        }],
        selected: 0,
        scroll: 0,
    });
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("resume"), "{text}");

    app.resume_picker = None;
    app.extension_view = true;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("Runtime extensions"), "{text}");
}

#[tokio::test]
async fn render_status_usage_without_window_and_zero() {
    let (mut app, _rx) = test_app().await;
    app.latest.usage.context_window = 0;
    app.latest.usage.total_input_tokens = 1234;
    app.latest.usage.output_tokens = 0;
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("1234 tok"), "human tokens: {text}");

    app.latest.usage.total_input_tokens = 0;
    app.latest.usage.output_tokens = 0;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        !text.contains("ctx"),
        "zero usage must not render a label: {text}"
    );
}

#[test]
fn prompt_chrome_covers_titles_features_placeholders_and_info_line() {
    let area = ratatui::layout::Rect::new(0, 0, 50, 4);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    let c = PromptChrome {
        focused: false,
        working_dir: Some("/root/project"),
        model_name: "provider:model",
        flags: &[PromptFlag {
            text: "2 queued",
            color: Color::Yellow,
            bold: true,
        }],
        usage: Some("12% ctx"),
        title: Some("A long session title that should be truncated"),
        features: &["graph engine".to_string(), "goal".to_string()],
        input_empty: true,
        ..Default::default()
    };
    let inner = render_prompt_chrome(
        &mut buf,
        area,
        &c,
        &crate::ui::theme::ComposerStyle::default(),
    );
    assert_eq!(inner.x, 4);
    assert!(inner.width > 0);
    assert!(inner.height > 0);
    let row = |y: u16| -> String {
        (0..area.width)
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol()))
            .collect()
    };
    assert!(row(3).contains("provider:model"), "{}", row(3));
    assert!(row(3).contains("2 queued"), "{}", row(3));
    assert!(row(3).contains("12% ctx"), "{}", row(3));
    assert!(row(0).contains("graph engine"), "{}", row(0));
    assert!(row(1).contains("Build any"), "{}", row(1));
}

#[test]
fn prompt_chrome_tiny_areas_and_info_without_usage() {
    let area = ratatui::layout::Rect::new(0, 0, 3, 2);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    let inner = render_prompt_chrome(
        &mut buf,
        area,
        &PromptChrome::default(),
        &crate::ui::theme::ComposerStyle::default(),
    );
    assert_eq!(inner, area);

    let area = ratatui::layout::Rect::new(0, 0, 40, 4);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    let c = PromptChrome {
        model_name: "m",
        working_dir: Some(" "),
        input_empty: false,
        ..Default::default()
    };
    render_prompt_chrome(
        &mut buf,
        area,
        &c,
        &crate::ui::theme::ComposerStyle::default(),
    );
    let row: String = (0..area.width)
        .filter_map(|x| buf.cell((x, 3)).map(|c| c.symbol()))
        .collect();
    assert!(row.contains("m"), "{row}");
}
