#[tokio::test]
async fn authoritative_snapshot_replaces_local_feed_annotations() {
    let (mut app, _rx) = test_app().await;
    let status = fixture_status(app.latest.feed_blocks.clone());
    app.system_line("local note");
    app.apply_snapshot(status);
    // Full frames replace the local render model even when the authoritative
    // transcript itself is unchanged.
    let text = feed_text(&app);
    assert!(!text.contains("local note"), "{text}");
}

#[tokio::test]
async fn snapshot_append_patch_pushes_only_new_block() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Plain {
        text: "banner".into(),
        level: theway_transport::feed::Level::System,
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    // Local annotations survive a pure tail append (no full rebuild).
    app.system_line("local note");
    let appended = WireFeedBlock::Assistant {
        text: "appended answer".into(),
        timestamp: None,
    };
    let mut second = fixture_status(Vec::new());
    second.feed_blocks_base = app.latest.feed_blocks.len() as u64;
    second.feed_block_patches = vec![WireFeedBlockPatch {
        index: second.feed_blocks_base,
        block: appended,
    }];
    app.apply_snapshot(second);
    let text = feed_text(&app);
    assert!(text.contains("banner"), "{text}");
    assert!(text.contains("appended answer"), "{text}");
    assert!(text.contains("local note"), "{text}");
}

#[tokio::test]
async fn snapshot_replacement_patch_updates_one_block() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Assistant {
        text: "partial".into(),
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    let mut patch = fixture_status(Vec::new());
    patch.feed_blocks_base = 1;
    patch.feed_block_patches = vec![WireFeedBlockPatch {
        index: 0,
        block: WireFeedBlock::Assistant {
            text: "complete".into(),
            timestamp: None,
        },
    }];

    app.apply_snapshot(patch);

    assert!(!app.resync_pending);
    assert_eq!(app.latest.feed_blocks.len(), 1);
    let text = feed_text(&app);
    assert!(text.contains("complete"), "{text}");
    assert!(!text.contains("partial"), "{text}");
}

#[tokio::test]
async fn snapshot_patch_gap_requests_authoritative_resync() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Assistant {
        text: "stable".into(),
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    let mut gap = fixture_status(Vec::new());
    gap.feed_blocks_base = 2;
    gap.feed_block_patches = vec![WireFeedBlockPatch {
        index: 2,
        block: WireFeedBlock::Assistant {
            text: "missed".into(),
            timestamp: None,
        },
    }];

    app.apply_snapshot(gap);

    assert!(app.resync_pending);
    assert_eq!(app.latest.feed_blocks.len(), 1);
    assert!(feed_text(&app).contains("stable"));
}

#[test]
fn headless_line_cursor_replays_after_transcript_shrink() {
    let mut printed = 5;

    let start = super::headless_unprinted_start(0, 2, &mut printed);

    assert_eq!(start, Some(0));
    assert_eq!(printed, 2);
    assert_eq!(super::headless_unprinted_start(2, 1, &mut printed), Some(0));
    assert_eq!(printed, 3);
    assert_eq!(super::headless_unprinted_start(2, 1, &mut printed), None);
}

#[tokio::test]
async fn snapshot_truncation_rebuilds_feed() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![
        WireFeedBlock::Plain {
            text: "one".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "two".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
    ]);
    app.apply_snapshot(first);
    // A shorter snapshot means the daemon truncated/reset the transcript —
    // prefix diff fails, the feed rebuilds from the new block list.
    let second = fixture_status(vec![WireFeedBlock::Plain {
        text: "fresh".into(),
        level: theway_transport::feed::Level::System,
        timestamp: None,
    }]);
    app.apply_snapshot(second);
    let text = feed_text(&app);
    assert!(text.contains("fresh"), "{text}");
    assert!(!text.contains("one"), "{text}");
    assert!(!text.contains("two"), "{text}");
}

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
        } => {
            assert_eq!(text, "hello daemon");
            assert!(images.is_empty());
            assert!(!interrupt);
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
async fn nonlocal_slash_forwards_to_daemon() {
    let (mut app, mut rx) = test_app().await;
    app.dispatch_slash("/model anthropic:claude-x", &mut terminal_placeholder())
        .await;
    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => {
            assert_eq!(text, "/model anthropic:claude-x")
        }
        other => panic!("unexpected command: {other:?}"),
    }
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
    assert!(matches!(cmd, WireCommand::Abort));
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
        WireCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn model_picker_alt_m_selects_and_sends_set_model() {
    let (mut app, mut rx) = test_app().await;
    app.open_model_picker();
    assert!(app.model_picker.is_some());

    // Enter descends into anthropic; Enter again selects the first model.
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
        WireCommand::SetModel { spec } => {
            assert_eq!(spec, "anthropic:claude-x")
        }
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(app.model_picker.is_none());
}

#[tokio::test]
async fn session_switch_sends_switch_session_rpc() {
    let (mut app, mut rx) = test_app().await;
    app.dispatch_slash("/session switch sess-1", &mut terminal_placeholder())
        .await;
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no switch_session command")
        .unwrap();
    match cmd {
        WireCommand::SwitchSession { id } => assert_eq!(id, "sess-1"),
        other => panic!("unexpected command: {other:?}"),
    }
}

/// Issue #52: `/new` creates a fresh session over the session-resource RPC
/// (`FakeSessionOps` ids come from a counter — the first create yields
/// `sess-new-1`) and switches to it. The gRPC create handler itself queues a
/// `SwitchSession` for the new id (becoming current is serialized through the
/// event loop), and the client-side switch queues a second one — both carry
/// the new id. The success line notes the new session id.
#[tokio::test]
async fn slash_new_creates_and_switches_session() {
    let (mut app, mut rx) = test_app().await;

    app.dispatch_slash("/new", &mut terminal_placeholder())
        .await;

    // Assert: both the create-time switch and the client-side switch arrive.
    for (i, origin) in ["create", "client-side switch"].iter().enumerate() {
        let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("no switch_session command")
            .unwrap();
        match cmd {
            WireCommand::SwitchSession { id } => assert_eq!(id, "sess-new-1"),
            other => panic!("unexpected command after {origin} (index {i}): {other:?}"),
        }
    }
    // Assert: the feed notes the new session id.
    let text = feed_text(&app);
    assert!(
        text.contains("new session sess-new-1"),
        "feed must note the new session id, got: {text}"
    );
}

/// Issue #52 failure path: when `create_session` errors, `/new` reports it as
/// an error line (the daemon never sees a forward).
#[tokio::test]
async fn slash_new_create_failure_shows_error_line() {
    let (mut app, rx) = test_app().await;
    // Dropping the command receiver closes the event-loop channel: the gRPC
    // create handler fails its SwitchSession enqueue with `unavailable`, so
    // `create_session` errors before any switch happens.
    drop(rx);

    app.dispatch_slash("/new", &mut terminal_placeholder())
        .await;

    // Assert: the create failure surfaces on the error line.
    let text = feed_text(&app);
    assert!(
        text.contains("error: create session failed"),
        "feed must show the create failure, got: {text}"
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

/// `/status-panel` opens the second-level menu (issue #54): Up/Down move
/// the highlight, Enter applies show/hide/auto and closes the menu, Esc
/// cancels without touching the mode.
#[tokio::test]
async fn status_panel_slash_opens_menu_and_keys_apply_or_cancel() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::empty()));

    // /status-panel opens the menu at option 0 (show).
    app.dispatch_slash("/status-panel", &mut term).await;
    assert_eq!(app.status_panel_menu, Some(0));

    // Down/Down highlight hide then auto; Up moves back.
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, Some(1));
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, Some(2));
    app.handle_event(key(KeyCode::Up), &mut term).await.unwrap();
    assert_eq!(app.status_panel_menu, Some(1));

    // Enter applies the highlighted mode (hide) and closes the menu.
    app.handle_event(key(KeyCode::Enter), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);

    // show → Shown(36).
    app.dispatch_slash("/status-panel", &mut term).await;
    app.handle_event(key(KeyCode::Enter), &mut term)
        .await
        .unwrap();
    assert_eq!(
        app.side_panel_mode,
        super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH)
    );

    // auto → Auto (Down Down Enter).
    app.dispatch_slash("/status-panel", &mut term).await;
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    app.handle_event(key(KeyCode::Enter), &mut term)
        .await
        .unwrap();
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Auto);

    // Esc cancels without touching the mode; the menu consumes unrelated
    // keys while open (typing never reaches the input).
    app.side_panel_mode = super::SidePanelMode::Shown(40);
    app.dispatch_slash("/status-panel", &mut term).await;
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    app.handle_event(key(KeyCode::Char('x')), &mut term)
        .await
        .unwrap();
    assert_eq!(app.input_text(), "", "menu must swallow typing");
    app.handle_event(key(KeyCode::Esc), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(40));
}

/// The `/status-panel` menu renders as a centered popup (issue #54): title
/// "status panel", the three options, and the popup highlight on the
/// selected option.
#[tokio::test]
async fn status_panel_menu_renders_centered_popup_with_highlight() {
    let (mut app, _rx) = test_app().await;
    app.status_panel_menu = Some(1); // "hide" highlighted
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let text = buffer_text(buf);

    assert!(text.contains("status panel"), "menu title missing:\n{text}");
    assert!(text.contains("show"), "menu options missing:\n{text}");
    assert!(text.contains("hide"), "menu options missing:\n{text}");
    assert!(text.contains("auto"), "menu options missing:\n{text}");

    // The highlighted option carries the popup's cyan background.
    let mut highlighted = Vec::new();
    for y in 0..buf.area().height {
        if (0..buf.area().width).any(|x| buf[(x, y)].bg == Color::Cyan) {
            highlighted.push(y);
        }
    }
    assert_eq!(
        highlighted.len(),
        1,
        "expected exactly one highlighted menu row, got rows {highlighted:?}"
    );
    let row: String = (0..buf.area().width)
        .map(|x| buf[(x, highlighted[0])].symbol())
        .collect::<String>()
        .trim_end()
        .to_string();
    assert!(
        row.contains("hide"),
        "highlight must sit on hide, row: {row:?}"
    );
}

/// Issue #55: `/fork` completes through the daemon-side command table
/// (`DAEMON_COMMANDS`), and the diff against
/// `Registry::with_daemon_commands()`'s auth surface stays complete
/// (`/login` `/logout` `/sessions`).
#[test]
fn collect_slash_commands_includes_daemon_fork_command() {
    // Arrange
    let registry = crate::local_commands::local_registry();

    // Act
    let commands = collect_slash_commands(&registry, &[], &[], &[]);

    // Assert
    assert!(
        commands.contains(&"/fork".to_string()),
        "completion list must contain /fork, got: {commands:?}"
    );
    assert!(commands.contains(&"/login".to_string()));
    assert!(commands.contains(&"/logout".to_string()));
    assert!(commands.contains(&"/sessions".to_string()));
}

/// Issue #55: bare `/fork` opens the interactive picker listing the current
/// session's User feed blocks NEWEST-first, numbered to match the daemon's
/// `/fork <n>` numbering (1 = most recent), with ≤60-char previews and
/// newlines flattened.
#[tokio::test]
async fn fork_picker_lists_feed_user_blocks_newest_first() {
    let (mut app, _rx) = test_app().await;
    // Arrange: oldest → newest, with non-User blocks interleaved.
    let long = format!("{}{}", "x".repeat(70), "\nsecond line");
    app.latest.feed_blocks = vec![
        WireFeedBlock::User {
            text: "oldest prompt".into(),
            timestamp: None,
        },
        WireFeedBlock::Assistant {
            text: "old answer".into(),
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "system note".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
        WireFeedBlock::User {
            text: long.clone(),
            timestamp: None,
        },
    ];

    // Act
    app.dispatch_slash("/fork", &mut terminal_placeholder())
        .await;

    // Assert: newest-first, 1-based, only User blocks, preview capped at
    // 60 chars + `…`, newlines flattened.
    let picker = app
        .fork_picker
        .as_ref()
        .expect("bare /fork must open the picker");
    assert_eq!(picker.entries.len(), 2);
    assert_eq!(picker.entries[0].number, 1);
    assert_eq!(picker.entries[1].number, 2);
    assert!(picker.entries[0].preview.starts_with(&"x".repeat(60)));
    assert!(
        picker.entries[0].preview.ends_with('…'),
        "over-long previews must be truncated with an ellipsis"
    );
    assert!(
        !picker.entries[0].preview.contains('\n'),
        "previews must flatten newlines for single-row rendering"
    );
    assert_eq!(picker.entries[1].preview, "oldest prompt");
}

/// Issue #55: Enter in the fork picker forwards `/fork <n>` for the
/// highlighted row's number (daemon numbering) and closes the popup; a
/// direct `/fork <n>` forwards immediately without opening the picker.
#[tokio::test]
async fn fork_picker_enter_forwards_slash_fork_number_and_arg_forwards_directly() {
    let (mut app, mut rx) = test_app().await;
    let mut term = terminal_placeholder();
    // Arrange: two user messages → newest gets number 1, older number 2.
    app.latest.feed_blocks = vec![
        WireFeedBlock::User {
            text: "first prompt".into(),
            timestamp: None,
        },
        WireFeedBlock::User {
            text: "second prompt".into(),
            timestamp: None,
        },
    ];
    app.dispatch_slash("/fork", &mut term).await;
    assert!(app.fork_picker.is_some());

    // Act: Down highlights number 2, Enter forwards `/fork 2`.
    app.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty())),
        &mut term,
    )
    .await
    .unwrap();
    app.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())),
        &mut term,
    )
    .await
    .unwrap();

    // Assert: the forwarded text carries the selected number; popup closed.
    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => assert_eq!(text, "/fork 2"),
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(app.fork_picker.is_none(), "Enter must close the picker");

    // Act: `/fork 1` with an argument skips the picker and forwards directly.
    app.dispatch_slash("/fork 1", &mut term).await;

    // Assert
    assert!(
        app.fork_picker.is_none(),
        "a /fork argument must not open the picker"
    );
    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => assert_eq!(text, "/fork 1"),
        other => panic!("unexpected command: {other:?}"),
    }
}

/// Issue #55: Esc cancels the fork picker without forwarding anything, and
/// a bare `/fork` on a feed without user messages reports the daemon's
/// error instead of opening an empty popup.
#[tokio::test]
async fn fork_picker_esc_cancels_and_empty_feed_reports_error() {
    let (mut app, mut rx) = test_app().await;
    let mut term = terminal_placeholder();
    app.latest.feed_blocks = vec![WireFeedBlock::User {
        text: "only prompt".into(),
        timestamp: None,
    }];
    app.dispatch_slash("/fork", &mut term).await;
    assert!(app.fork_picker.is_some());

    // Act: Esc cancels.
    app.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
        &mut term,
    )
    .await
    .unwrap();

    // Assert: closed, nothing forwarded.
    assert!(app.fork_picker.is_none(), "Esc must cancel the picker");
    assert!(
        rx.try_recv().is_err(),
        "Esc must not forward any command to the daemon"
    );

    // Act: no user blocks → bare /fork reports the error, no popup.
    app.latest.feed_blocks = vec![];
    app.dispatch_slash("/fork", &mut term).await;

    // Assert
    assert!(app.fork_picker.is_none());
    let text = feed_text(&app);
    assert!(
        text.contains("error: no user messages to fork from"),
        "feed must report the empty-feed fork error, got: {text}"
    );
}

/// The fork picker renders as a centered popup (issue #55): a "fork" title,
/// the newest-first user rows, and the completion-popup highlight on the
/// selected row.
#[tokio::test]
async fn fork_picker_renders_centered_popup_with_fork_title() {
    let (mut app, _rx) = test_app().await;
    app.latest.feed_blocks = vec![
        WireFeedBlock::User {
            text: "oldest prompt".into(),
            timestamp: None,
        },
        WireFeedBlock::User {
            text: "newest prompt".into(),
            timestamp: None,
        },
    ];
    app.dispatch_slash("/fork", &mut terminal_placeholder())
        .await;
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let text = buffer_text(buf);

    assert!(text.contains("fork"), "popup title missing:\n{text}");
    assert!(
        text.contains("1) newest prompt"),
        "newest user row missing:\n{text}"
    );
    assert!(
        text.contains("2) oldest prompt"),
        "oldest user row missing:\n{text}"
    );

    // The selected row carries the completion popup's cyan background.
    let mut highlighted = Vec::new();
    for y in 0..buf.area().height {
        if (0..buf.area().width).any(|x| buf[(x, y)].bg == Color::Cyan) {
            highlighted.push(y);
        }
    }
    assert_eq!(
        highlighted.len(),
        1,
        "expected exactly one highlighted picker row, got rows {highlighted:?}"
    );
    let row: String = (0..buf.area().width)
        .map(|x| buf[(x, highlighted[0])].symbol())
        .collect::<String>()
        .trim_end()
        .to_string();
    assert!(
        row.contains("1) newest prompt"),
        "highlight must sit on the first row, row: {row:?}"
    );
}
