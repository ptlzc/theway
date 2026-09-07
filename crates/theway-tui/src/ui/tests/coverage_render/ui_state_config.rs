use super::*;

#[test]
fn ui_state_load_precedence_and_render_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let state = dir.path().join("ui-state.toml");
    crate::config_payload::set_config_path_for_tests(Some(config.clone()));
    crate::ui_state::set_state_path_for_tests(Some(state.clone()));

    // No files -> defaults.
    assert_eq!(crate::ui_state::load(), Default::default());

    // Legacy state file fallback when config has no [ui].
    std::fs::write(
            &state,
            "[feed]\nthinking_mode = \"peek\"\n[panel]\nmode = \"shown\"\n[graph]\nposition = \"side-panel\"\n",
        )
        .unwrap();
    let loaded = crate::ui_state::load();
    assert_eq!(loaded.thinking_mode, Some(ThinkingMode::Peek));

    // Config [ui] namespace wins.
    std::fs::write(
            &config,
            "[ui.feed]\nthinking_mode = \"hidden\"\n[ui.panel]\nmode = \"hidden\"\n[ui.graph]\nposition = \"composer-top\"\n",
        )
        .unwrap();
    let loaded = crate::ui_state::load();
    assert_eq!(loaded.thinking_mode, Some(ThinkingMode::Hidden));
    assert_eq!(loaded.panel_mode, Some(crate::ui::SidePanelMode::Hidden));
    assert_eq!(
        loaded.graph_position,
        Some(crate::ui::GraphPosition::ComposerTop)
    );

    // Render all fields and round-trip.
    let all = crate::ui_state::UiState {
        thinking_mode: Some(ThinkingMode::Full),
        panel_mode: Some(crate::ui::SidePanelMode::Shown(
            crate::ui::TRIGGER_PANEL_WIDTH,
        )),
        panel_position: Some(crate::ui::SidePanelPosition::Left),
        graph_position: Some(crate::ui::GraphPosition::ComposerTop),
        show_hooks: true,
        show_runtime: true,
    };
    let text = crate::ui_state::render(&all);
    assert!(text.contains("thinking_mode = \"full\""));
    assert!(text.contains("mode = \"shown\""));
    assert!(text.contains("position = \"left\""));
    assert!(text.contains("show_hooks = true"));
    assert!(text.contains("show_runtime = true"));
    assert!(text.contains("[graph]"));
    assert_eq!(crate::ui_state::load_from_config_text(&text), all);

    crate::config_payload::set_config_path_for_tests(None);
    crate::ui_state::set_state_path_for_tests(None);
}

#[test]
fn ui_state_save_empty_removes_and_defaults_from_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.toml");
    std::fs::write(&path, "old").unwrap();
    crate::ui_state::save_to(&path, &crate::ui_state::UiState::default()).unwrap();
    assert!(!path.exists());
    assert_eq!(crate::ui_state::load_from(&path), Default::default());
    assert_eq!(
        crate::ui_state::load_from(&dir.path().join("missing")),
        Default::default()
    );
}

#[tokio::test]
async fn config_payload_assemble_covers_missing_readable_and_unreadable() {
    use crate::cli::Cli;
    use clap::Parser as _;

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let existing = dir.path().join("config.toml");
    std::fs::write(
        &existing,
        "[model]\nprovider = \"acme\"\nmodel = \"warp-9\"\n[tui]\nmax_feed_lines = 8000\n",
    )
    .unwrap();
    let cwd = std::path::Path::new("/tmp/cwd");

    let cli = Cli::parse_from(["theway"]);
    crate::config_payload::set_config_path_for_tests(Some(missing.clone()));
    let (payload, notes) = assemble_config(&cli, cwd).await;
    assert!(notes.is_empty(), "{notes:?}");
    assert!(payload.provider.is_none());

    crate::config_payload::set_config_path_for_tests(Some(existing.clone()));
    let (payload, notes) = assemble_config(&cli, cwd).await;
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(payload.provider.as_deref(), Some("acme"));
    assert_eq!(payload.model.as_deref(), Some("warp-9"));
    assert_eq!(payload.tui_max_feed_lines, Some(8000));

    // A directory is read-unreadable and produces a diagnostic while
    // still returning a CLI-only payload.
    crate::config_payload::set_config_path_for_tests(Some(dir.path().to_path_buf()));
    let (payload, notes) = assemble_config(&cli, cwd).await;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("config: cannot read"), "{notes:?}");
    assert!(payload.provider.is_none());

    crate::config_payload::set_config_path_for_tests(None);
}

#[test]
fn config_payload_reconcile_clears_all_optional_fields_and_storage_notes() {
    let current = WireDaemonConfig {
        provider: Some("a".into()),
        model: Some("m".into()),
        base_url: Some("http://x".into()),
        thinking: Some(true),
        thinking_level: Some("high".into()),
        builtin_skills: vec!["old".into()],
        skills_dirs: vec!["/old".into()],
        skills: vec![theway_transport::wire::WireProvisionedSkill {
            name: "s".into(),
            description: String::new(),
            content: String::new(),
            file_path: "/s".into(),
            source: "user".into(),
            disable_model_invocation: false,
        }],
        templates: vec![theway_transport::wire::WireProvisionedTemplate {
            name: "t".into(),
            description: String::new(),
            content: String::new(),
            file_path: "/t".into(),
        }],
        mcp_servers: vec![theway_transport::wire::WireProvisionedMcpServer {
            name: "mcp".into(),
            kind: "stdio".into(),
            command: Some("cmd".into()),
            args: vec![],
            endpoint: None,
            auth: None,
            request_timeout_ms: None,
            sse_idle_timeout_ms: None,
            body_cap_bytes: None,
            reconnect: None,
            inject_summary: false,
            inject_and_run: false,
        }],
        trigger_poll_secs: Some(10),
        tui_max_feed_lines: Some(20),
        tool_service_addr: Some("127.0.0.1:1".into()),
        storage_service_addr: Some("127.0.0.1:2".into()),
        ..Default::default()
    };
    let desired = WireDaemonConfig {
        clear_fields: vec![
            "base_url".into(),
            "thinking".into(),
            "thinking_level".into(),
            "builtin_skills".into(),
            "skills_dirs".into(),
            "skills".into(),
            "templates".into(),
            "mcp_servers".into(),
            "trigger_poll_secs".into(),
            "tui_max_feed_lines".into(),
            "tool_service_addr".into(),
        ],
        ..Default::default()
    };
    let (patch, _notes) = reconcile(&desired, &current, true);
    assert!(patch.clear_fields.iter().any(|f| f == "base_url"));
    assert!(patch.clear_fields.iter().any(|f| f == "builtin_skills"));
    assert!(patch.clear_fields.iter().any(|f| f == "skills_dirs"));
    assert!(patch.clear_fields.iter().any(|f| f == "skills"));
    assert!(patch.clear_fields.iter().any(|f| f == "templates"));
    assert!(patch.clear_fields.iter().any(|f| f == "mcp_servers"));
    assert!(patch.clear_fields.iter().any(|f| f == "trigger_poll_secs"));
    assert!(patch.clear_fields.iter().any(|f| f == "tui_max_feed_lines"));
    assert!(patch.clear_fields.iter().any(|f| f == "tool_service_addr"));
    assert!(
        !patch
            .clear_fields
            .iter()
            .any(|f| f == "storage_service_addr")
    );

    // Attach asks for a different storage endpoint: mismatch note, no patch.
    let mut desired2 = WireDaemonConfig::default();
    desired2.storage_service_addr = Some("127.0.0.1:9".into());
    let (_, notes) = reconcile(&desired2, &current, true);
    assert!(
        notes.iter().any(|n| n.contains("storage service")),
        "{notes:?}"
    );

    // Direct clear_field helper avoids duplicate entries.
    let mut patch = WireDaemonConfig::default();
    clear_field(&mut patch, "x");
    clear_field(&mut patch, "x");
    assert_eq!(patch.clear_fields, vec!["x".to_string()]);
}
