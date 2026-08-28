#[tokio::test]
async fn model_spec_uses_typed_switch_while_model_list_forwards_to_daemon() {
    let (mut app, rx) = test_app().await;
    let (_drain, seen) = drain_commands(rx);
    app.dispatch_slash("/model anthropic:claude-x", &mut terminal_placeholder())
        .await;
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["SetModel(anthropic:claude-x)"]
    );

    app.dispatch_slash("/model list", &mut terminal_placeholder())
        .await;
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["SetModel(anthropic:claude-x)", "Submit(/model list)"]
    );
}

#[tokio::test]
async fn model_picker_descends_three_levels_and_emits_model_plus_thinking() {
    let (mut app, rx) = test_app().await;
    let (_drain, seen) = drain_commands(rx);
    app.open_model_picker();
    assert!(app.model_picker.is_some());

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
    // provider → model list
    assert!(app.handle_model_picker_key(&enter).await);
    // model list → thinking intensity
    assert!(app.handle_model_picker_key(&enter).await);
    // thinking intensity → selection (model + thinking commands)
    assert!(app.handle_model_picker_key(&enter).await);

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["SetModel(anthropic:claude-x)", "SetThinking(off)"]
    );
    assert!(app.model_picker.is_none());
}

#[tokio::test]
async fn model_picker_confirmed_snapshot_persists_startup_defaults() {
    let (mut app, rx) = test_app().await;
    let (_drain, seen) = drain_commands(rx);
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    app.model_config_path = config_path.clone();
    app.open_model_picker();
    assert!(app.model_picker.is_some());

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
    assert!(app.handle_model_picker_key(&enter).await);
    assert!(app.handle_model_picker_key(&enter).await);
    // Select a non-default thinking level (cursor: current = off → move to high).
    for _ in 0..4 {
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()))
            .await;
    }
    assert!(app.handle_model_picker_key(&enter).await);
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["SetModel(anthropic:claude-x)", "SetThinking(high)"]
    );
    assert!(app.model_picker.is_none());
    assert!(
        !config_path.exists(),
        "RPC admission must not persist before snapshot confirmation"
    );

    let mut unrelated = fixture_status(Vec::new());
    unrelated.model = "provider:other".into();
    unrelated.thinking_level = "off".into();
    app.apply_snapshot(unrelated);
    assert!(!config_path.exists());

    let mut confirmed = fixture_status(Vec::new());
    confirmed.model = "anthropic:claude-x".into();
    confirmed.thinking_level = "high".into();
    app.apply_snapshot(confirmed);
    let text = std::fs::read_to_string(config_path).unwrap();
    assert_eq!(
        theway_transport::config::parse_model_default(&text).unwrap(),
        Some(theway_transport::config::ModelDefault {
            provider: "anthropic".into(),
            model: "claude-x".into(),
        })
    );
    assert_eq!(
        theway_transport::config::parse_model_thinking_default(&text).unwrap(),
        Some("high".into())
    );
}

#[tokio::test]
async fn model_snapshot_malformed_config_keeps_bytes_and_reports_error() {
    let (mut app, rx) = test_app().await;
    let (_drain, _seen) = drain_commands(rx);
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    let malformed = b"[model\nprovider = \"broken\"\n";
    std::fs::write(&config_path, malformed).unwrap();
    app.model_config_path = config_path.clone();

    app.set_model_from_spec("anthropic:claude-x").await;
    let mut confirmed = fixture_status(Vec::new());
    confirmed.model = "anthropic:claude-x".into();
    app.apply_snapshot(confirmed);

    assert_eq!(std::fs::read(config_path).unwrap(), malformed);
    assert!(
        feed_text(&app).contains("could not save the startup default"),
        "{}",
        feed_text(&app)
    );
}

#[tokio::test]
async fn model_picker_hides_providers_without_credentials() {
    let (mut app, mut rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.model_catalog = vec![
        theway_transport::wire::ProviderGroup {
            provider: "anthropic".into(),
            has_credential: true,
            models: vec![theway_transport::wire::ModelEntry {
                id: "claude-x".into(),
                name: "Claude X".into(),
            }],
        },
        theway_transport::wire::ProviderGroup {
            provider: "openai".into(),
            has_credential: false,
            models: vec![theway_transport::wire::ModelEntry {
                id: "gpt-x".into(),
                name: "GPT X".into(),
            }],
        },
    ];
    app.apply_snapshot(status);

    app.open_model_picker();
    let picker = app.model_picker.as_ref().expect("picker open");
    assert_eq!(picker.groups.len(), 1);
    assert_eq!(picker.groups[0].provider, "anthropic");
    let (_, rows) = picker.view(10);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].0.contains("anthropic"));

    // The openai group never appears, so selection can only reach anthropic.
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
    app.handle_model_picker_key(&enter).await;
    let picker = app.model_picker.as_ref().expect("picker still open");
    let (title, rows) = picker.view(10);
    assert_eq!(title, "anthropic models");
    assert_eq!(rows.len(), 1);
    assert!(rx.try_recv().is_err(), "no command until thinking level chosen");
}
