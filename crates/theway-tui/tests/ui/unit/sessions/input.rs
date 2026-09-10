#[tokio::test]
async fn submit_sends_message_to_daemon() {
    let (mut app, mut rx) = test_app().await;
    app.set_input("hello daemon");
    app.submit(&mut terminal_placeholder()).await.unwrap();
    match rx.recv().await.unwrap() {
        WireCommand::Submit {
            text,
            images,
            interrupt,
            ..
        } => {
            assert_eq!(text, "hello daemon");
            assert!(images.is_empty());
            assert!(!interrupt);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

/// The skill envelope is still built client-side and wraps the verbatim
/// user text.
#[tokio::test]
async fn submit_keeps_skill_envelope_around_raw_text() {
    let (mut app, mut rx) = test_app().await;
    app.pending_skill = Some("git".into());
    app.set_input("look at @src/foo.rs");
    app.submit(&mut terminal_placeholder()).await.unwrap();
    assert!(app.pending_skill.is_none(), "the pending skill is consumed");
    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => {
            assert!(
                text.contains("invoke the Skill tool with name \"git\""),
                "{text}"
            );
            assert!(
                text.ends_with("User request:\nlook at @src/foo.rs"),
                "the user text is appended verbatim: {text}"
            );
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

/// Mentions are the daemon's to resolve, exactly once at admission: the
/// client submits the user's text verbatim, so an `@path` token survives
/// unchanged and no `Files in context:` block is appended client-side.
#[tokio::test]
async fn submit_sends_raw_text_without_expanding_mentions() {
    let (mut app, mut rx) = test_app().await;
    // The mention resolves against a real file: the removed client-side
    // expansion would have appended this file's content to the prompt.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/foo.rs"), "fn main() {}\n").unwrap();
    app.cwd = dir.path().to_path_buf();

    app.set_input("look at @src/foo.rs");
    app.submit(&mut terminal_placeholder()).await.unwrap();

    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => {
            assert_eq!(
                text.matches("@src/foo.rs").count(),
                1,
                "the `@path` token must travel verbatim exactly once: {text}"
            );
            assert!(!text.contains("Files in context:"), "{text}");
            assert!(!text.contains("<file path="), "{text}");
            assert!(
                !text.contains("fn main()"),
                "the file body must not be inlined client-side: {text}"
            );
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn slash_quit_sets_quit_and_clear_empties_feed() {
    let (mut app, _rx) = test_app().await;
    app.dispatch_slash("/quit", &mut terminal_placeholder())
        .await;
    assert!(app.quit);

    app.feed.push_user("stale");
    app.dispatch_slash("/clear", &mut terminal_placeholder())
        .await;
    assert!(feed_text(&app).is_empty());
}

#[tokio::test]
async fn ctrl_c_while_busy_sends_cancel() {
    let (mut app, mut rx) = test_app().await;
    app.busy = true;
    app.request_abort();
    // cancel is fired on a spawned task; drain the command channel.
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no cancel command")
        .unwrap();
    assert!(matches!(cmd, WireCommand::Abort { .. }));
}

#[tokio::test]
async fn second_ctrl_c_during_abort_force_quits() {
    let (mut app, _rx) = test_app().await;
    app.busy = true;

    app.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut terminal_placeholder(),
    )
    .await
    .unwrap();
    assert!(app.abort_requested);
    assert!(!app.quit, "the first Ctrl-C only requests the abort");

    app.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut terminal_placeholder(),
    )
    .await
    .unwrap();
    assert!(app.quit, "the second Ctrl-C must force-quit the TUI");
}

#[tokio::test]
async fn control_plane_prompt_key_approves_via_rpc() {
    let (mut app, mut rx) = test_app().await;
    app.control_plane_prompt = Some(theway_transport::wire::WireControlPlanePromptSnapshot {
        tool_name: "write".into(),
        label: "write file".into(),
        reason: "needs approval".into(),
        args_hash: "abc".into(),
        payload: "{}".into(),
    });
    assert!(
        app.handle_control_plane_prompt_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
    );
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no approve command")
        .unwrap();
    match cmd {
        WireCommand::ResolveControlPlane { approve, .. } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn session_switch_selects_session_locally() {
    let (mut app, _rx) = test_app().await;
    app.dispatch_slash("/session switch sess-1", &mut terminal_placeholder())
        .await;
    assert_eq!(app.session_id, "sess-1");
    let text = feed_text(&app);
    assert!(text.contains("selected session sess-1"), "{text}");
}

/// Issue #52: `/new` creates a fresh session over the session-resource RPC
/// (`FakeSessionOps` ids come from a counter — the first create yields
/// `sess-new-1`) and selects it client-side. No switch command is sent.
#[tokio::test]
async fn slash_new_creates_and_selects_session() {
    let (mut app, _rx) = test_app().await;

    app.dispatch_slash("/new", &mut terminal_placeholder())
        .await;

    // Assert: the new session is selected client-side.
    assert_eq!(app.session_id, "sess-new-1");
    // Assert: the feed notes the new session id.
    let text = feed_text(&app);
    assert!(
        text.contains("new session sess-new-1"),
        "feed must note the new session id, got: {text}"
    );
}

/// Session creation is now client-side and no longer requires the daemon
/// switch command channel; `/new` creates and selects the session directly.
#[tokio::test]
async fn slash_new_create_succeeds_without_switch_channel() {
    let (mut app, rx) = test_app().await;
    // Dropping the command receiver closes the old switch-command channel;
    // creation still succeeds because it no longer depends on that channel.
    drop(rx);

    app.dispatch_slash("/new", &mut terminal_placeholder())
        .await;

    // Assert: the new session is selected client-side and noted in the feed.
    assert_eq!(app.session_id, "sess-new-1");
    let text = feed_text(&app);
    assert!(
        text.contains("new session sess-new-1"),
        "feed must note the new session id, got: {text}"
    );
}

/// Issue #52: `/new` completes as a TUI-local command (`LOCAL_COMMANDS`) and
/// stays out of the daemon-side command table the client forwards.
#[test]
fn collect_slash_commands_includes_local_new_command() {
    // Arrange
    let registry = crate::local_commands::local_registry();

    // Act
    let commands = collect_slash_commands(&registry, &[], &[], &[]);

    // Assert
    assert!(
        commands.contains(&"/new".to_string()),
        "completion list must contain /new, got: {commands:?}"
    );
    assert!(
        !super::DAEMON_COMMANDS.contains(&"new"),
        "/new is TUI-local and must not live in the daemon command table"
    );
}

/// Issue #54: `/status-panel` completes as a TUI-local command
/// (`LOCAL_COMMANDS`) and stays out of the daemon-side command table the
/// client forwards.
#[test]
fn collect_slash_commands_includes_local_status_panel_command() {
    // Arrange
    let registry = crate::local_commands::local_registry();

    // Act
    let commands = collect_slash_commands(&registry, &[], &[], &[]);

    // Assert
    assert!(
        commands.contains(&"/status-panel".to_string()),
        "completion list must contain /status-panel, got: {commands:?}"
    );
    assert!(
        !super::DAEMON_COMMANDS.contains(&"status-panel"),
        "/status-panel is TUI-local and must not live in the daemon command table"
    );
}
