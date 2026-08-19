/// Side-panel mode render resolution (issue #54): every mode shares the
/// ≥100-column gate; Auto keeps the content-driven rule, Hidden closes,
/// Shown(w) forces the panel (content or not) and clamps the width to
/// [SIDE_PANEL_MIN_WIDTH, content - 40].
#[test]
fn resolve_side_panel_width_applies_mode_gate_and_clamps() {
    use super::SidePanelMode as Mode;

    // The ≥100-column gate hides the panel in every mode.
    for mode in [Mode::Auto, Mode::Shown(36), Mode::Hidden] {
        assert_eq!(
            super::resolve_side_panel_width(mode, true, 99),
            None,
            "below 100 columns the panel must stay hidden ({mode:?})"
        );
    }

    // Auto: the panel content decides.
    assert_eq!(
        super::resolve_side_panel_width(Mode::Auto, true, 140),
        Some(super::TRIGGER_PANEL_WIDTH)
    );
    assert_eq!(
        super::resolve_side_panel_width(Mode::Auto, false, 140),
        None,
        "Auto without panel content hides the panel"
    );

    // Hidden: always closed.
    assert_eq!(
        super::resolve_side_panel_width(Mode::Hidden, true, 140),
        None
    );

    // Shown: forced even without content; width clamps to [24, content-40].
    assert_eq!(
        super::resolve_side_panel_width(Mode::Shown(36), false, 140),
        Some(36)
    );
    assert_eq!(
        super::resolve_side_panel_width(Mode::Shown(10), true, 140),
        Some(super::SIDE_PANEL_MIN_WIDTH),
        "widths below the floor clamp up to 24"
    );
    assert_eq!(
        super::resolve_side_panel_width(Mode::Shown(200), true, 140),
        Some(100),
        "upper clamp = content width - 40"
    );
    // At exactly 100 columns the clamp ceiling is 60.
    assert_eq!(
        super::resolve_side_panel_width(Mode::Shown(80), true, 100),
        Some(60)
    );
}

/// Panel render per mode (issue #54): Auto renders the default 36-wide
/// panel when it has content; Hidden drops it and clears the recorded rect;
/// Shown forces it even without content at the clamped width.
#[tokio::test]
async fn side_panel_render_follows_mode_auto_hidden_shown() {
    let (mut app, _rx) = test_app().await;
    let backend = TestBackend::new(120, 20);
    let mut terminal = Terminal::new(backend).unwrap();

    // Auto + content: the 36-wide panel renders and shrinks the feed area.
    app.latest.sidebar.skills.items = vec![WireSkillSnapshot {
        name: "code-review".into(),
        source: "user".into(),
        file_path: "/skills/code-review".into(),
        enabled: true,
    }];
    terminal.draw(|f| app.render(f)).unwrap();
    let panel = app
        .last_panel_area
        .expect("auto mode with content renders the panel");
    assert_eq!(panel.width, super::TRIGGER_PANEL_WIDTH);
    assert_eq!(app.last_feed_area.unwrap().width, 120 - 36);
    assert!(
        buffer_text(terminal.backend().buffer()).contains("Skills"),
        "panel content missing"
    );

    // Hidden: no panel, the feed reclaims the full width, the recorded
    // rect is cleared so a stale area never matches a grab.
    app.side_panel_mode = super::SidePanelMode::Hidden;
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(app.last_panel_area.is_none());
    assert_eq!(app.last_feed_area.unwrap().width, 120);

    // Shown forces the panel even without content; widths clamp up to 24.
    app.latest.sidebar.skills.items.clear();
    app.side_panel_mode = super::SidePanelMode::Shown(10);
    terminal.draw(|f| app.render(f)).unwrap();
    let panel = app
        .last_panel_area
        .expect("shown mode renders the panel regardless of content");
    assert_eq!(panel.width, super::SIDE_PANEL_MIN_WIDTH);
    assert_eq!(app.last_feed_area.unwrap().width, 120 - 24);
}

/// Panel left-edge drag (issue #54): grabbing the border column starts a
/// width drag that exits Auto; the width tracks start + (start_col - col),
/// clamps to [24, 60], and hides when the pointer reaches the panel's right
/// edge or squeezes below 24; release ends the drag.
#[tokio::test]
async fn panel_edge_drag_resizes_clamps_and_drag_past_right_hides() {
    let (mut app, _rx) = test_app().await;
    app.latest.sidebar.skills.items = vec![WireSkillSnapshot {
        name: "code-review".into(),
        source: "user".into(),
        file_path: "/skills/code-review".into(),
        enabled: true,
    }];
    let backend = TestBackend::new(140, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let area = app.last_panel_area.unwrap();
    assert_eq!(area.width, 36);

    // Grab the 1-column left-edge strip: the drag anchors at the rendered
    // width and the mode switches from Auto to Shown.
    app.handle_mouse_down(mouse_event(
        area.x,
        area.y + 1,
        MouseEventKind::Down(MouseButton::Left),
    ));
    let drag = app.panel_drag.expect("edge grab starts a panel drag");
    assert_eq!((drag.start_col, drag.start_width), (area.x, 36));
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(36));

    // Dragging left grows the panel (right edge anchored).
    app.handle_mouse_drag(area.x - 6, area.y + 1);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(42));

    // Dragging far left clamps at the 60-column ceiling.
    app.handle_mouse_drag(area.x - 40, area.y + 1);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(60));

    // Squeezing below 24 hides the panel…
    app.handle_mouse_drag(area.x + 20, area.y + 1);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);

    // …but dragging back left within the same drag reopens it.
    app.handle_mouse_drag(area.x - 4, area.y + 1);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(40));

    // Reaching the panel's right edge (its last column) hides it.
    let right = area.x + area.width - 1;
    app.handle_mouse_drag(right, area.y + 1);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);

    // Release ends the drag; the grab never started a feed selection.
    app.handle_mouse_up().await;
    assert!(app.panel_drag.is_none());
    assert!(app.feed_selection.is_none());
}

#[tokio::test]
async fn mouse_drag_selects_feed_lines() {
    let (mut app, _rx) = test_app().await;
    for i in 0..20 {
        app.feed
            .push_plain_untimed(format!("row-{i}"), theway_transport::feed::Level::Output);
    }
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let feed = app.last_feed_area.unwrap();
    let anchor_row = feed.y + 2;
    let top = app.selection_view.top;
    app.handle_mouse_down(mouse_event(
        feed.x + 2,
        anchor_row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    let sel = app.feed_selection.unwrap();
    assert_eq!(
        sel.anchor,
        (top + 2, 2),
        "down must anchor the clicked cell (row, display column)"
    );
    app.handle_mouse_drag(feed.x + 2, anchor_row + 3);
    let sel = app.feed_selection.unwrap();
    assert_eq!(
        sel.head,
        (top + 5, 2),
        "drag must extend the head over the rows crossed"
    );
    app.handle_mouse_up().await;
    assert!(
        app.feed_selection.is_some(),
        "selection persists after the button is released"
    );
    assert!(!app.mouse_selecting);
}

/// Mouse column mapping (issue #53): the anchor takes the display column
/// within the row; a click past the row end clamps to the row's text width
/// (terminal semantics); the drag updates the head's row and column.
#[tokio::test]
async fn mouse_down_maps_display_column_and_clamps_to_row_width() {
    let (mut app, _rx) = test_app().await;
    app.feed
        .push_plain_untimed("abcdef", theway_transport::feed::Level::Output);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let feed = app.last_feed_area.unwrap();
    let row = feed.y + 1; // "banner" is row 0, "abcdef" is row 1

    app.handle_mouse_down(mouse_event(
        feed.x + 2,
        row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    assert_eq!(
        app.feed_selection.unwrap().anchor,
        (app.selection_view.top + 1, 2),
        "click inside the text maps to the display column"
    );
    app.handle_mouse_drag(feed.x + 4, row);
    assert_eq!(
        app.feed_selection.unwrap().head,
        (app.selection_view.top + 1, 4),
        "drag updates the head's row and column"
    );

    // Release after a real drag copies and keeps the selection.
    app.handle_mouse_up().await;
    assert!(app.feed_selection.is_some());

    // A plain click past the row end clamps to the text width...
    app.handle_mouse_down(mouse_event(
        feed.x + 30,
        row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    assert_eq!(
        app.feed_selection.unwrap().anchor.1,
        6,
        "click past the row end clamps to the text width"
    );

    // ...and releasing without a drag clears the zero-width selection.
    app.handle_mouse_up().await;
    assert!(app.feed_selection.is_none());
}

/// Ctrl+Space selects the visible page; Shift+arrows extend per char / row /
/// page; Esc clears (issue #53).
#[tokio::test]
async fn ctrl_space_selects_page_and_shift_keys_extend_by_char_row_page() {
    let (mut app, _rx) = test_app().await;
    for i in 0..30 {
        app.feed.push_plain_untimed(
            format!("line-{i:02}"),
            theway_transport::feed::Level::Output,
        );
    }
    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let mut term = terminal_placeholder();
    let press = |key: KeyEvent| Event::Key(key);

    // Ctrl+Space: (view.top, 0) → (view.bottom, last-row text width).
    app.handle_event(
        press(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
        &mut term,
    )
    .await
    .unwrap();
    let view = app.selection_view;
    let sel = app.feed_selection.unwrap();
    assert_eq!(
        sel.anchor,
        (view.top, 0),
        "anchor = first visible row, column 0"
    );
    assert_eq!(
        sel.head,
        (view.bottom, 7),
        "head = last visible row, its text width"
    );
    assert!(
        view.bottom >= view.top,
        "the visible page spans at least one row"
    );

    // Shift+←/→ move one column, clamped to the row width.
    for _ in 0..2 {
        app.handle_event(
            press(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)),
            &mut term,
        )
        .await
        .unwrap();
    }
    assert_eq!(app.feed_selection.unwrap().head, (view.bottom, 5));
    app.handle_event(
        press(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.feed_selection.unwrap().head, (view.bottom, 6));

    // Shift+↑/↓ move one row, keeping the column.
    app.handle_event(
        press(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(
        app.feed_selection.unwrap().head,
        (view.bottom.saturating_sub(1), 6)
    );
    app.handle_event(
        press(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.feed_selection.unwrap().head, (view.bottom, 6));

    // Shift+PgUp/PgDn move one page.
    let page = app.last_viewport_h.max(1);
    app.handle_event(
        press(KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT)),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(
        app.feed_selection.unwrap().head,
        (view.bottom.saturating_sub(page), 6)
    );
    app.handle_event(
        press(KeyEvent::new(KeyCode::PageDown, KeyModifiers::SHIFT)),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.feed_selection.unwrap().head, (view.bottom, 6));

    // Esc clears.
    app.handle_event(
        press(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.feed_selection.is_none());
}

/// Copy invocation (issue #53): mouse release and Ctrl+Shift+C both push
/// the extracted text through the clipboard sink and report
/// `copied N chars · M lines`.
#[tokio::test]
async fn copy_selection_reports_chars_and_lines_through_mock_handler() {
    let (mut app, _rx) = test_app().await;
    let captured: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let sink = captured.clone();
    app.copy_handler = Some(Arc::new(move |text| {
        *sink.lock().unwrap() = text;
        true
    }));
    app.feed
        .push_plain_untimed("alpha", theway_transport::feed::Level::Output);
    app.feed
        .push_plain_untimed("beta", theway_transport::feed::Level::Output);
    app.feed
        .push_plain_untimed("gamma", theway_transport::feed::Level::Output);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let feed = app.last_feed_area.unwrap();

    // Mouse release after a drag copies the selected cells.
    app.handle_mouse_down(mouse_event(
        feed.x,
        feed.y + 1,
        MouseEventKind::Down(MouseButton::Left),
    ));
    app.handle_mouse_drag(feed.x + 5, feed.y + 3);
    app.handle_mouse_up().await;
    assert_eq!(*captured.lock().unwrap(), "alpha\nbeta\ngamma");
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("copied 16 chars · 3 lines"),
        "system line must report the copy:\n{text}"
    );

    // Ctrl+Shift+C copies explicitly and keeps the selection.
    let mut term = terminal_placeholder();
    app.handle_event(
        Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(*captured.lock().unwrap(), "alpha\nbeta\ngamma");
    assert!(
        app.feed_selection.is_some(),
        "explicit copy must keep the selection (Esc clears)"
    );
}

/// Keyboard scroll acceleration (issue #38): consecutive same-direction
/// Press/Repeat events ramp the multiplier 1.0 → 1.1 → … capped at 1.5x;
/// a direction change or a key Release resets the chain to 1.0x.
#[tokio::test]
async fn keyboard_scroll_acceleration_mult_sequence_caps_and_resets() {
    let (mut app, _rx) = test_app().await;

    // Multiplier ramp: +0.1 per same-direction repeat, capped at 1.5x.
    let expected = [1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.5, 1.5];
    for (repeat, want) in expected.into_iter().enumerate() {
        let got = App::scroll_key_mult(repeat as u32);
        assert!(
            (got - want).abs() < 1e-9,
            "repeat {repeat}: mult {got}, want {want}"
        );
    }

    // Step = base × mult: same-direction presses ramp, the sixth event
    // reaches the cap and further repeats stay capped.
    let base = 20;
    let steps: Vec<usize> = (0..8).map(|_| app.scroll_key_step(false, base)).collect();
    assert_eq!(steps, vec![20, 22, 24, 26, 28, 30, 30, 30]);
    assert_eq!(app.scroll_repeat, 7);
    assert_eq!(app.scroll_repeat_up, Some(false));

    // Direction change resets the chain: the new direction starts at 1.0x.
    assert_eq!(app.scroll_key_step(true, base), 20);
    assert_eq!(app.scroll_repeat, 0);
    assert_eq!(app.scroll_repeat_up, Some(true));

    // A key Release resets the chain end-to-end through the event loop.
    let mut term = terminal_placeholder();
    let release = Event::Key(KeyEvent {
        kind: KeyEventKind::Release,
        ..KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)
    });
    app.handle_event(release, &mut term).await.unwrap();
    assert_eq!(app.scroll_repeat, 0);
    assert_eq!(app.scroll_repeat_up, None);
    // Next press after the release is a fresh 1.0x first press.
    assert_eq!(app.scroll_key_step(true, base), 20);
}

/// Composer wheel browsing (issue #38): the wheel over the textarea rect is
/// forwarded to the textarea (multi-line draft browsing) and never scrolls
/// the feed; the wheel over the feed region scrolls the feed at the plain
/// SCROLL_STEP and never touches the textarea.
#[tokio::test]
async fn mouse_wheel_routes_between_composer_textarea_and_feed() {
    let (mut app, _rx) = test_app().await;
    for i in 0..40 {
        app.feed
            .push_plain_untimed(format!("row-{i}"), theway_transport::feed::Level::Output);
    }
    // A draft taller than the MAX_INPUT_ROWS cap gives the textarea
    // overflow it can scroll through.
    app.set_input(
        &(0..10)
            .map(|i| format!("draft line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text_area = app.last_text_area.unwrap();
    let feed = app.last_feed_area.unwrap();

    // Unpin from the bottom anchor so feed scroll deltas are observable.
    app.scroll_up(10);
    terminal.draw(|f| app.render(f)).unwrap();
    let feed_scroll = app.scroll;
    // Cursor sits at the draft end, so the textarea viewport starts
    // bottom-anchored (scroll > 0).
    let ta_scroll = app.input_state.scroll;
    assert!(
        ta_scroll > 0,
        "a 10-line draft in a capped composer must start scrolled down"
    );

    // Wheel over the composer text area: the textarea scrolls, the feed
    // does not move.
    app.handle_mouse_event(mouse_event(
        text_area.x + 1,
        text_area.y + 1,
        MouseEventKind::ScrollUp,
    ))
    .await;
    assert_eq!(
        app.scroll, feed_scroll,
        "wheel over the composer must not scroll the feed"
    );
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(
        app.input_state.scroll < ta_scroll,
        "wheel over the composer must scroll the draft view"
    );

    // Wheel over the feed region: the feed scrolls by one plain SCROLL_STEP
    // and the textarea view is untouched.
    let ta_scroll = app.input_state.scroll;
    app.handle_mouse_event(mouse_event(
        feed.x + 2,
        feed.y + 2,
        MouseEventKind::ScrollDown,
    ))
    .await;
    assert_eq!(
        app.scroll,
        feed_scroll + super::SCROLL_STEP,
        "feed wheel keeps the plain SCROLL_STEP (no acceleration)"
    );
    terminal.draw(|f| app.render(f)).unwrap();
    assert_eq!(
        app.input_state.scroll, ta_scroll,
        "wheel over the feed must not touch the textarea view"
    );
}

/// Feed scrollback cap (issue #27 + #34): the render cache drains the head
/// lazily — only once the line count exceeds the cap by the internal margin —
/// and shifts the scroll offset by the trimmed count so a scrolled-up view
/// does not jump.
#[test]
fn feed_cache_trims_to_cap_and_tracks_trimmed() {
    let mut feed = theway_transport::feed::Feed::new();
    for i in 0..4_000 {
        feed.push_plain_untimed(format!("row-{i}"), theway_transport::feed::Level::Output);
    }
    let mut cache = crate::feed_cache::FeedRenderCache::new();
    let opts = crate::feed_render::FeedRenderOptions::default();
    cache.update(&feed, 80, &opts, super::DEFAULT_MAX_FEED_LINES);
    assert_eq!(cache.trimmed(), 1_000);
    assert_eq!(cache.lines().len(), super::DEFAULT_MAX_FEED_LINES);
    assert_eq!(cache.lines()[0].spans[0].content, "row-1000");
    assert_eq!(
        cache.lines()[super::DEFAULT_MAX_FEED_LINES - 1].spans[0].content,
        "row-3999"
    );

    // Under the cap: no-op, nothing trimmed.
    let mut feed = theway_transport::feed::Feed::new();
    for i in 0..10 {
        feed.push_plain_untimed(format!("row-{i}"), theway_transport::feed::Level::Output);
    }
    let mut cache = crate::feed_cache::FeedRenderCache::new();
    cache.update(&feed, 80, &opts, super::DEFAULT_MAX_FEED_LINES);
    assert_eq!(cache.trimmed(), 0);
    assert_eq!(cache.lines().len(), 10);
}

#[tokio::test]
async fn feed_render_caps_scrollback_at_default_max_lines() {
    let (mut app, _rx) = test_app().await;
    // ~3.2k rendered rows: one Plain block with one short row per line (no
    // wrapping at width 50).
    app.feed.clear();
    let rows: Vec<String> = (0..3_200).map(|i| format!("row-{i:04}")).collect();
    app.feed
        .push_plain_untimed(rows.join("\n"), theway_transport::feed::Level::Note);

    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    // Following: the capped tail is visible (newest row on screen) and the
    // uncapped scroll anchor sits at the bottom of the full feed.
    app.follow = true;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-3199"),
        "follow must show the newest feed row:\n{text}"
    );
    assert_eq!(
        app.scroll,
        3_200 - app.last_viewport_h,
        "follow anchors scroll one viewport above the uncapped end"
    );

    // Scrolled up: the view must keep showing the same content (uncapped
    // offset 2000 → display offset 1800 after the 200-line head trim), and a
    // second draw must not drift the view.
    app.follow = false;
    app.scroll = 2_000;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-2000"),
        "scrolled-up view must keep showing the same feed content:\n{text}"
    );
    assert_eq!(app.scroll, 2_000, "uncapped scroll anchor must not change");
    assert!(!app.follow, "scrolled up must disable follow");

    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-2000"),
        "second draw must not drift the scrolled-up view:\n{text}"
    );
}

/// The daemon-pushed `tui_max_feed_lines` config value overrides the
/// built-in 3000-line scrollback cap (issue #27 follow-up).
#[tokio::test]
async fn feed_render_uses_tui_max_feed_lines_from_snapshot() {
    let (mut app, _rx) = test_app().await;
    app.feed.clear();
    let rows: Vec<String> = (0..3_200).map(|i| format!("row-{i:04}")).collect();
    let block = theway_transport::feed::WireFeedBlock::Plain {
        text: rows.join("\n"),
        level: theway_transport::feed::Level::Note,
        timestamp: None,
    };
    app.feed
        .push_plain_untimed(rows.join("\n"), theway_transport::feed::Level::Note);
    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    // Cap above the feed size: nothing trimmed — the top of the scrollback
    // still shows original row 0.
    let mut status = fixture_status(vec![block.clone()]);
    status.tui_max_feed_lines = Some(4_000);
    app.apply_snapshot(status);
    app.follow = false;
    app.scroll = 0;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-0000"),
        "cap 4000 must keep row 0:\n{text}"
    );

    // Cap below the feed size: head-trimmed to 1000. Scrolling to the very
    // top reveals the oldest kept row (original row 2200), and following
    // still shows the newest.
    let mut status = fixture_status(vec![block.clone()]);
    status.tui_max_feed_lines = Some(1_000);
    app.apply_snapshot(status);
    app.follow = false;
    app.scroll = 0;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-2200"),
        "cap 1000: the top of the scrollback must be original row 2200:\n{text}"
    );
    app.follow = true;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-3199"),
        "cap 1000 must keep the newest row:\n{text}"
    );

    // Zero / absent from the snapshot falls back to the built-in 3000-line
    // default. The head trim drains lazily (only past cap + margin, issue
    // #34), so 3200 lines stay fully visible: the top of the scrollback is
    // original row 0.
    let mut status = fixture_status(vec![block.clone()]);
    status.tui_max_feed_lines = Some(0);
    app.apply_snapshot(status);
    app.follow = false;
    app.scroll = 0;
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("row-0000"),
        "zero cap must fall back to the 3000-line default (lazy trim keeps the extra rows):\n{text}"
    );
}

#[tokio::test]
async fn snapshot_rebuilds_feed_and_resyncs_busy_panel() {
    let (mut app, _rx) = test_app().await;
    assert!(
        crate::feed_render::lines(
            &app.feed,
            100,
            &crate::feed_render::FeedRenderOptions::default()
        )
        .iter()
        .any(|l| { l.spans.iter().any(|s| s.content.contains("banner")) })
    );

    let mut status = fixture_status(vec![
        WireFeedBlock::User {
            text: "snap question".into(),
            timestamp: None,
        },
        WireFeedBlock::Assistant {
            text: "snap answer".into(),
            timestamp: None,
        },
    ]);
    status.busy = true;
    status.queued_count = 2;
    // Scrolled-up view: a snapshot append must NOT yank the view back to the
    // bottom (issue #33 — follow is user-controlled, not snapshot-forced).
    app.follow = false;
    app.apply_snapshot(status);

    assert!(app.busy);
    assert_eq!(app.latest.queued_count, 2);
    let text = feed_text(&app);
    assert!(text.contains("❯ snap question"), "{text}");
    assert!(text.contains("ai ▸ snap answer"), "{text}");
    // The old banner block is gone (whole-replacement semantics).
    assert!(!text.contains("banner"), "{text}");
    assert!(!app.follow, "snapshot append must not re-enable follow");
}
