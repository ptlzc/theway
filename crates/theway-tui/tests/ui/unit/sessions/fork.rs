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
