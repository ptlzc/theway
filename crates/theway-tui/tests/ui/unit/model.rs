#[tokio::test]
async fn model_spec_uses_typed_switch_while_model_list_forwards_to_daemon() {
    let (mut app, mut rx) = test_app().await;
    app.dispatch_slash("/model anthropic:claude-x", &mut terminal_placeholder())
        .await;
    match rx.recv().await.unwrap() {
        WireCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-x"),
        other => panic!("unexpected command: {other:?}"),
    }

    app.dispatch_slash("/model list", &mut terminal_placeholder())
        .await;
    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => assert_eq!(text, "/model list"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn model_picker_confirmed_snapshot_persists_startup_default() {
    let (mut app, mut rx) = test_app().await;
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    app.model_config_path = config_path.clone();
    app.open_model_picker();
    assert!(app.model_picker.is_some());

    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no set_model command")
        .unwrap();
    match cmd {
        WireCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-x"),
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(app.model_picker.is_none());
    assert!(
        !config_path.exists(),
        "RPC admission must not persist before snapshot confirmation"
    );

    let mut unrelated = fixture_status(Vec::new());
    unrelated.model = "provider:other".into();
    app.apply_snapshot(unrelated);
    assert!(!config_path.exists());

    let mut confirmed = fixture_status(Vec::new());
    confirmed.model = "anthropic:claude-x".into();
    app.apply_snapshot(confirmed);
    let text = std::fs::read_to_string(config_path).unwrap();
    assert_eq!(
        theway_transport::config::parse_model_default(&text).unwrap(),
        Some(theway_transport::config::ModelDefault {
            provider: "anthropic".into(),
            model: "claude-x".into(),
        })
    );
}

#[tokio::test]
async fn model_snapshot_malformed_config_keeps_bytes_and_reports_error() {
    let (mut app, mut rx) = test_app().await;
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    let malformed = b"[model\nprovider = \"broken\"\n";
    std::fs::write(&config_path, malformed).unwrap();
    app.model_config_path = config_path.clone();

    app.set_model_from_spec("anthropic:claude-x").await;
    assert!(matches!(
        rx.recv().await.unwrap(),
        WireCommand::SetModel { .. }
    ));
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
