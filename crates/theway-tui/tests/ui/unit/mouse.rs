/// Mouse wheel scrolling: each notch moves the feed 3 lines, scrolling up
/// detaches follow, and non-scroll mouse events are inert.
#[tokio::test]
async fn wheel_scrolls_feed_and_other_mouse_events_are_inert() {
    let (mut app, _rx) = test_app().await;
    app.scroll = 20;
    app.follow = true;
    let wheel_up = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollUp,
        column: 1,
        row: 1,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let wheel_down = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollDown,
        column: 1,
        row: 1,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: 1,
        row: 1,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };

    app.handle_mouse(wheel_up);
    assert_eq!(app.scroll, 17, "one wheel notch scrolls up 3 lines");
    assert!(!app.follow, "scrolling up detaches follow");
    app.handle_mouse(wheel_down);
    app.handle_mouse(wheel_down);
    assert_eq!(app.scroll, 23, "wheel down scrolls 3 lines per notch");
    app.handle_mouse(click);
    assert_eq!(app.scroll, 23, "non-scroll mouse events are inert");
}

// ── mouse character selection + OSC 52 copy (issue #70) ────────────────────

fn mouse_event(kind: crossterm::event::MouseEventKind, row: u16, column: u16) -> crossterm::event::MouseEvent {
    crossterm::event::MouseEvent {
        kind,
        row,
        column,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

/// Left-button press/drag/release over the feed selects the dragged
/// characters, keeps them highlighted, and emits an OSC 52 clipboard payload
/// on release.
#[tokio::test]
async fn mouse_left_drag_selects_characters_and_copies_via_osc52() {
    let (mut app, _rx) = test_app().await;
    let status = fixture_status(vec![
        WireFeedBlock::Plain {
            text: "row A".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "row B".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "row C".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
    ]);
    app.apply_snapshot(status);
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    // Pin the geometry the mouse handler hit-tests against.
    app.last_feed_area = Some(ratatui::layout::Rect::new(0, 0, 80, 10));
    app.last_display_scroll = 0;

    // Press at row 1 column 4 (0-based, crossterm coordinates) -> line 1,
    // sub-column 4 ("B" of "row B").
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        1,
        4,
    ));
    let sel = app.mouse_select.expect("press starts a selection");
    assert_eq!(sel.anchor, super::MousePos { line: 1, col: 4 });
    assert_eq!(sel.current, super::MousePos { line: 1, col: 4 });
    assert!(sel.dragging);

    // Drag down to row 3 column 9 -> line 2, col 9 (past "row C"'s end).
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        3,
        9,
    ));
    let sel = app.mouse_select.expect("drag extends the selection");
    assert_eq!(sel.anchor, super::MousePos { line: 1, col: 4 });
    assert_eq!(sel.current, super::MousePos { line: 2, col: 9 });
    assert_eq!(app.selected_text(), "B\nrow C");

    // Release: selection stays (highlighted), OSC 52 payload emitted.
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        3,
        9,
    ));
    let sel = app.mouse_select.expect("release keeps the selection");
    assert!(!sel.dragging, "drag ended");
    let bytes = app.selection_bytes().expect("selection yields OSC 52 bytes");
    let prefix = b"\x1b]52;c;";
    assert!(bytes.starts_with(prefix), "OSC 52 prefix: {bytes:?}");
    assert_eq!(*bytes.last().unwrap(), 0x07, "BEL terminator");
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&bytes[prefix.len()..bytes.len() - 1])
        .unwrap();
    assert_eq!(
        String::from_utf8(decoded).unwrap(),
        "B\nrow C",
        "clipboard payload decodes to the selected text"
    );
}

/// A plain click (press+release without dragging) clears any selection and
/// copies nothing; a press outside the feed pane also clears.
#[tokio::test]
async fn mouse_click_and_outside_press_clear_selection() {
    let (mut app, _rx) = test_app().await;
    app.apply_snapshot(fixture_status(vec![
        WireFeedBlock::Plain {
            text: "row A".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "row B".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
    ]));
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    app.last_feed_area = Some(ratatui::layout::Rect::new(0, 0, 80, 10));
    app.last_display_scroll = 0;

    // Drag a selection, then a plain click clears it.
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        2,
        5,
    ));
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        3,
        5,
    ));
    assert!(app.mouse_select.is_some());
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        2,
        5,
    ));
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        2,
        5,
    ));
    assert!(app.mouse_select.is_none(), "click without drag clears");

    // A press outside the feed pane clears too.
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        2,
        5,
    ));
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        3,
        5,
    ));
    assert!(app.mouse_select.is_some());
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        20, // below the 10-row feed pane
        5,
    ));
    assert!(app.mouse_select.is_none(), "outside press clears");
}

/// Wheel scrolling clears a live selection (row indices shift under it).
#[tokio::test]
async fn mouse_wheel_clears_selection() {
    let (mut app, _rx) = test_app().await;
    app.apply_snapshot(fixture_status(vec![WireFeedBlock::Plain {
        text: "row A".into(),
        level: theway_transport::feed::Level::Output,
        timestamp: None,
    }]));
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    app.last_feed_area = Some(ratatui::layout::Rect::new(0, 0, 80, 10));
    app.last_display_scroll = 0;
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        2,
        5,
    ));
    assert!(app.mouse_select.is_some());
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::ScrollUp,
        1,
        1,
    ));
    assert!(app.mouse_select.is_none(), "wheel scroll clears selection");
}


// ── mouse selection across regions (issue #103) ────────────────────────────

/// Press/drag over the side panel, composer and status bar selects and copies
/// text from those regions (feed selection is covered by issue #70 tests).
#[tokio::test]
async fn mouse_selects_panel_composer_and_status_regions() {
    let (mut app, _rx) = test_app().await;

    // Pin geometry + snapshotted lines for the non-feed regions.
    let panel_area = ratatui::layout::Rect::new(60, 0, 20, 10);
    let input_area = ratatui::layout::Rect::new(0, 11, 80, 4);
    let status_area = ratatui::layout::Rect::new(0, 9, 80, 1);
    app.last_panel_area = Some(panel_area);
    app.last_input_text_area = Some(input_area);
    app.last_status_area = Some(status_area);
    app.last_feed_area = Some(ratatui::layout::Rect::new(0, 0, 59, 8));
    app.panel_select_lines = vec![
        ratatui::text::Line::raw("Session".to_string()),
        ratatui::text::Line::raw("…78dc · unnamed".to_string()),
        ratatui::text::Line::raw("Skills".to_string()),
    ];
    app.status_select_lines = vec![ratatui::text::Line::raw(" ready ".to_string())];
    app.set_input("hello world");

    // Panel: press at (row 1, col 61) -> panel line 1, col 1 ("…78dc…").
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        1,
        61,
    ));
    let sel = app.mouse_select.expect("panel press starts a selection");
    assert_eq!(sel.region, super::SelectRegion::Panel);
    assert_eq!(sel.anchor, super::MousePos { line: 1, col: 1 });
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        2,
        66,
    ));
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        2,
        66,
    ));
    assert!(
        app.selected_text().contains("78dc · unnamed"),
        "panel selection text: {:?}",
        app.selected_text()
    );

    // Status: press at (row 9, col 3) -> status col 3 ("ready" region).
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        9,
        3,
    ));
    let sel = app.mouse_select.expect("status press starts a selection");
    assert_eq!(sel.region, super::SelectRegion::Status);

    // Composer: press/drag over the input text.
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        11,
        0,
    ));
    app.handle_mouse(mouse_event(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        11,
        5,
    ));
    let sel = app.mouse_select.expect("composer drag keeps selection");
    assert_eq!(sel.region, super::SelectRegion::Composer);
    assert_eq!(app.selected_text(), "hello", "composer selection text");
}
