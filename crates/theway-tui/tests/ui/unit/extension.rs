#[test]
fn extension_contribution_renderer_uses_known_kinds_and_ignores_unknown() {
    let extensions = theway_transport::wire::WireExtensionSnapshot {
        contributions: vec![
            theway_transport::wire::WireExtensionContribution {
                contribution_id: "status".into(),
                extension_id: "example.extension".into(),
                scope: "session".into(),
                kind: "status_item".into(),
                payload: serde_json::json!({"label": "Anchor", "value": "promoted"}),
            },
            theway_transport::wire::WireExtensionContribution {
                contribution_id: "future".into(),
                extension_id: "example.extension".into(),
                scope: "session".into(),
                kind: "future_renderer".into(),
                payload: serde_json::json!({"executable": false}),
            },
        ],
        ..Default::default()
    };
    let lines = crate::ui::extension_contribution_lines(&extensions);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans[0].content, "Anchor: promoted");
}

#[tokio::test]
async fn runtime_extension_view_renders_redaction_without_feed_noise() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.extensions = theway_transport::wire::WireExtensionSnapshot {
        revision: 2,
        catalog: vec![theway_transport::wire::WireExtensionCatalogEntry {
            extension_id: "anchor".into(),
            version: "1.0.0".into(),
            source: "project".into(),
            scope: "session".into(),
            priority: 0,
            status: "faulted".into(),
            permissions: Vec::new(),
            reason_code: Some("hook_failed".into()),
        }],
        diagnostics: vec![theway_transport::wire::WireExtensionDiagnostic {
            extension_id: "anchor".into(),
            code: "hook_failed".into(),
            severity: "error".into(),
            message: "bootstrap failed".into(),
            session_id: None,
            event: None,
            sequence: None,
            details: serde_json::Map::new(),
            redacted_fields: vec!["authorization".into()],
        }],
        ..Default::default()
    };
    app.apply_snapshot(status);
    assert!(app.feed.blocks().is_empty(), "status must not append feed blocks");
    app.extension_view = true;
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("anchor 1.0.0 [faulted"), "{text}");
    assert!(text.contains("redacted: authorization"), "{text}");
    assert!(!text.contains("ai ▸"), "extension view must not synthesize assistant content");
}
