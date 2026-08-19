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
