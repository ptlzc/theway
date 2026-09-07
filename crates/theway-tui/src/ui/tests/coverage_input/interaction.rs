use super::*;

// ── interaction.rs edge paths ────────────────────────────────────────

#[tokio::test]
async fn handle_event_covers_key_release_paste_mouse_and_unknown() {
    let (mut app, _rx) = test_app().await;
    let mut terminal = terminal_placeholder();

    // Key release resets the scroll repeat.
    app.scroll_repeat = 3;
    app.scroll_repeat_up = Some(true);
    app.handle_event(
        Event::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())
        }),
        &mut terminal,
    )
    .await
    .unwrap();
    assert_eq!(app.scroll_repeat, 0);
    assert!(app.scroll_repeat_up.is_none());

    // Paste inserts text (short plain path).
    app.handle_event(Event::Paste("pasted".into()), &mut terminal)
        .await
        .unwrap();
    assert_eq!(app.input_text(), "pasted");

    // Mouse "other" clears a selection.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Feed,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 1 },
        dragging: true,
    });
    app.handle_event(
        Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            row: 0,
            column: 0,
            modifiers: KeyModifiers::empty(),
        }),
        &mut terminal,
    )
    .await
    .unwrap();
    assert!(
        app.mouse_select.is_none(),
        "unknown mouse kind clears selection"
    );

    // Unknown event is ignored.
    app.handle_event(Event::Resize(80, 24), &mut terminal)
        .await
        .unwrap();
}

#[tokio::test]
async fn mouse_drag_left_handles_missing_selection_and_region_changes() {
    let (mut app, _rx) = test_app().await;
    let mouse =
        |kind: crossterm::event::MouseEventKind, row: u16, col: u16| crossterm::event::MouseEvent {
            kind,
            row,
            column: col,
            modifiers: KeyModifiers::empty(),
        };
    app.last_feed_area = Some(ratatui::layout::Rect::new(0, 0, 20, 10));
    app.last_display_scroll = 0;

    // No drag and no selection is a no-op.
    app.mouse_drag_left(mouse(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        1,
        1,
    ));
    assert!(app.mouse_select.is_none());

    // Existing selection but !dragging is a no-op.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Feed,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 0 },
        dragging: false,
    });
    app.mouse_drag_left(mouse(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        1,
        1,
    ));
    assert_eq!(
        app.mouse_select.unwrap().current,
        crate::ui::MousePos { line: 0, col: 0 }
    );

    // Dragging outside a selectable region does not update the selection.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Feed,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 0 },
        dragging: true,
    });
    app.mouse_drag_left(mouse(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        20,
        30,
    ));
    assert_eq!(
        app.mouse_select.unwrap().current,
        crate::ui::MousePos { line: 0, col: 0 }
    );

    // Dragging into a different region leaves the original anchor intact.
    app.last_panel_area = Some(ratatui::layout::Rect::new(0, 0, 20, 10));
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Panel,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 0 },
        dragging: true,
    });
    app.mouse_drag_left(mouse(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        1,
        1,
    ));
    assert_eq!(
        app.mouse_select.unwrap().region,
        crate::ui::SelectRegion::Panel
    );

    // A valid same-region drag updates current.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Feed,
        anchor: crate::ui::MousePos { line: 1, col: 0 },
        current: crate::ui::MousePos { line: 1, col: 0 },
        dragging: true,
    });
    app.mouse_drag_left(mouse(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        2,
        3,
    ));
    assert_eq!(
        app.mouse_select.unwrap().current,
        crate::ui::MousePos { line: 0, col: 3 }
    );
}

#[tokio::test]
async fn mouse_up_left_handles_click_and_empty_selection() {
    let (mut app, _rx) = test_app().await;

    // Active panel drag ends without touching a text selection.
    app.last_panel_area = Some(ratatui::layout::Rect::new(100, 0, 36, 10));
    app.side_panel_position = crate::ui::SidePanelPosition::Right;
    app.handle_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        row: 1,
        column: 100,
        modifiers: KeyModifiers::empty(),
    });
    assert!(app.panel_drag.is_some());
    app.mouse_up_left();
    assert!(app.panel_drag.is_none());

    // No selection: no-op.
    app.mouse_select = None;
    app.mouse_up_left();

    // Non-dragging selection: no-op.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Composer,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 1 },
        dragging: false,
    });
    app.mouse_up_left();
    assert!(app.mouse_select.is_some());

    // Anchor == current (plain click): clears.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Feed,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 0 },
        dragging: true,
    });
    app.mouse_up_left();
    assert!(app.mouse_select.is_none());

    // Selection with empty copyable text: bytes are None, selection cleared.
    app.mouse_select = Some(crate::ui::MouseSelect {
        region: crate::ui::SelectRegion::Status,
        anchor: crate::ui::MousePos { line: 0, col: 0 },
        current: crate::ui::MousePos { line: 0, col: 0 },
        dragging: true,
    });
    app.mouse_select.as_mut().unwrap().current.col = 1;
    app.status_select_lines = vec![];
    app.mouse_up_left();
    assert!(app.mouse_select.is_none());
}

#[tokio::test]
async fn scroll_key_step_and_composer_row_branches() {
    let (mut app, _rx) = test_app().await;
    app.set_input("short");
    assert!(app.input_is_single_line());
    app.set_input("line1\nline2\n");
    assert!(!app.input_is_single_line());

    // composer_rows: in-range and over-limit re-measure/clamp paths.
    assert!(app.composer_rows(80) >= 1);
    assert!(app.composer_rows(20) >= 1);
}

#[tokio::test]
async fn scroll_key_step_async_branches() {
    let (mut app, _rx) = test_app().await;
    app.last_viewport_h = 10;

    // First press in a direction starts at 1.0x.
    let step = app.scroll_key_step(true, 10);
    assert!(step >= 10);
    assert_eq!(app.scroll_repeat, 0);
    assert_eq!(app.scroll_repeat_up, Some(true));

    // Same direction increments repeat and accelerates.
    let step2 = app.scroll_key_step(true, 10);
    assert_eq!(app.scroll_repeat, 1);
    assert!(step2 >= step);

    // Direction change resets the repeat.
    app.scroll_key_step(false, 10);
    assert_eq!(app.scroll_repeat, 0);
    assert_eq!(app.scroll_repeat_up, Some(false));
}

#[tokio::test]
async fn feed_and_region_pos_edge_cases() {
    let (mut app, _rx) = test_app().await;
    app.last_feed_area = Some(ratatui::layout::Rect::new(2, 1, 10, 5));
    app.last_display_scroll = 7;
    app.last_panel_area = None;
    app.last_input_text_area = None;
    app.last_status_area = None;
    let feed = app.feed_pos_at(1, 2);
    assert!(feed.is_some());
    // The fixture feed cache is empty, so the scroll-aware line index
    // clamps to the last (0th) rendered line.
    assert_eq!(feed.unwrap(), crate::ui::MousePos { line: 0, col: 0 });
    assert!(app.feed_pos_at(0, 2).is_none(), "above feed");
    assert!(app.feed_pos_at(1, 1).is_none(), "left of feed");
    assert!(app.feed_pos_at(99, 99).is_none(), "outside feed");
    // When no non-feed area is present, region_pos_at still maps feed.
    assert!(app.region_pos_at(1, 2).is_some());
    // And outside all areas returns None.
    assert!(app.region_pos_at(99, 99).is_none());

    // Feed missing entirely -> None.
    app.last_feed_area = None;
    assert!(app.feed_pos_at(1, 2).is_none());
    assert!(app.region_pos_at(1, 2).is_none());
}

#[tokio::test]
async fn region_selection_lines_across_all_regions() {
    let (mut app, _rx) = test_app().await;
    app.feed_cache.lines();
    app.set_input("composer\ntext");
    app.panel_select_lines = vec![ratatui::text::Line::raw("panel")];
    app.status_select_lines = vec![ratatui::text::Line::raw("status")];
    assert!(app.region_lines(crate::ui::SelectRegion::Composer).len() == 2);
    assert_eq!(
        app.region_lines(crate::ui::SelectRegion::Panel)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "panel"
    );
    assert_eq!(
        app.region_lines(crate::ui::SelectRegion::Status)[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>(),
        "status"
    );
    assert_eq!(
        app.region_lines(crate::ui::SelectRegion::Feed).len(),
        app.feed_cache.lines().len()
    );
    // selected_text with no selection is empty.
    assert_eq!(app.selected_text(), "");
}

#[tokio::test]
async fn input_display_lines_counts_chips_and_tail() {
    let (mut app, _rx) = test_app().await;
    app.set_input("hello");
    assert_eq!(app.input_display_lines(), 1);
    app.set_input("a\nb");
    assert_eq!(app.input_display_lines(), 2);
    app.insert_paste_text("x\ny\nz\nw".into());
    assert!(app.input_display_lines() >= 1);
}
