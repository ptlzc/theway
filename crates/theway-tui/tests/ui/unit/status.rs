// ── thinking stats wiring (issue #44) ─────────────────────────────────────

/// The thinking stats line's right side renders the live c/s meter plus the
/// recent turn's in/out token counts from the snapshot usage — previously
/// `thinking_cps` / `thinking_output_tokens` were never assigned and the line
/// was stuck at `c/s: 0 · output: 0` with no input.
#[tokio::test]
async fn thinking_stats_line_shows_cps_and_in_out_tokens() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(vec![WireFeedBlock::Thinking {
        text: "pondering the design".into(),
        timestamp: None,
    }]);
    status.usage = WireContextUsage {
        input_tokens: 57_100,
        output_tokens: 1_200,
        ..Default::default()
    };
    app.apply_snapshot(status);
    // Seed the char/s meter: 500 bytes over 0.5 s → 1000 char/s.
    let now = std::time::Instant::now();
    app.cps_meter.record_at(now - Duration::from_millis(500), 0);
    app.cps_meter.record_at(now, 500);
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("c/s: 1000 · in: 57.1k · out: 1.2k"),
        "thinking stats line missing live counters:\n{text}"
    );
}

// ── busy Braille spinner (issue #140) ─────────────────────────────────────

#[test]
fn braille_frames_match_pi_order_and_repeat_in_one_cell() {
    let expected = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    assert_eq!(snake_loader::BRAILLE_FRAMES, expected);
    let cycle = expected.len() as u64;
    for (step, glyph) in expected.into_iter().enumerate() {
        let frame = snake_loader::braille_frame(step as u64);
        assert_eq!(frame.glyph, glyph, "step {step}");
        assert_eq!(
            unicode_width::UnicodeWidthChar::width(frame.glyph),
            Some(1),
            "frame {glyph} must stay in one terminal cell"
        );
        assert_eq!(
            snake_loader::braille_frame(step as u64 + cycle),
            frame,
            "step {step} must repeat after one ten-frame cycle"
        );
    }
}

#[test]
fn braille_frames_follow_the_pi_rainbow_hues() {
    let mut previous = None;
    for step in 0..snake_loader::BRAILLE_FRAMES.len() {
        let hue = step as f32 / snake_loader::BRAILLE_FRAMES.len() as f32 * 300.0;
        let (r, g, b) = super::pixel_loader::hsv_to_rgb(hue, 0.95, 1.0);
        let frame = snake_loader::braille_frame(step as u64);
        assert_eq!(frame.fg, Color::Rgb(r, g, b), "step {step}");
        if let Some(previous) = previous {
            assert_ne!(frame.fg, previous, "adjacent frames need distinct hues");
        }
        previous = Some(frame.fg);
    }
}

#[tokio::test]
async fn local_display_toggles_do_not_append_operation_logs() {
    let (mut app, _rx) = test_app().await;
    let before = feed_text(&app);

    app.cycle_thinking_mode();
    app.toggle_tool_outputs();

    assert_eq!(feed_text(&app), before);
}

#[tokio::test]
async fn slash_popup_navigates_with_arrows_and_accepts_with_enter() {
    let (mut app, _rx) = test_app().await;
    app.set_input("/");
    assert!(app.completions.len() > 1, "bare slash lists commands");
    assert_eq!(app.completion_idx, 0);
    app.completion_next();
    assert_eq!(app.completion_idx, 1);
    app.completion_prev();
    assert_eq!(app.completion_idx, 0);
    let expected = app.completions[1].clone();
    app.completion_next();
    app.accept_completion();
    assert_eq!(
        app.input_text(),
        expected,
        "Enter accepts the highlighted entry"
    );
    assert!(
        app.completions.is_empty(),
        "popup closes once the accepted command matches exactly"
    );
    // History keys are untouched when the popup is closed: arrows still
    // navigate history on a single-line draft.
    app.set_input("");
    assert!(app.completions.is_empty());
    app.completion_next();
    assert_eq!(app.completion_idx, 0);
}

/// Locate the single highlighted popup row (cyan background) in a rendered
/// buffer: its text and row. The popup is the only cyan-background surface,
/// so a stray second highlighted row fails the scan.
fn highlighted_popup_cell(buf: &Buffer) -> (String, u16) {
    let mut rows = Vec::new();
    for y in 0..buf.area().height {
        if (0..buf.area().width).any(|x| buf[(x, y)].bg == Color::Cyan) {
            rows.push(y);
        }
    }
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one highlighted completion row, got rows {rows:?}"
    );
    let y = rows[0];
    let text: String = (0..buf.area().width)
        .map(|x| buf[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string();
    (text, y)
}

/// Assert the highlight sits on `expected` AND its row is inside the popup's
/// item rows (between the title border and the bottom border), i.e. the
/// selection is actually visible in the rendered window.
fn assert_highlight_on_popup(app: &App, buf: &Buffer, expected: &str) {
    let (symbol, y) = highlighted_popup_cell(buf);
    assert!(
        symbol.contains(expected),
        "highlight must sit on the selected match {expected:?}, row text: {symbol:?}"
    );
    let status_y = app.last_status_area.unwrap().y;
    let shown = app.completions.len().min(super::COMPLETION_POPUP_MAX);
    let height = shown as u16 + 2;
    assert!(
        y > status_y.saturating_sub(height) && y <= status_y.saturating_sub(2),
        "highlight row {y} must stay inside the popup window above status row {status_y}"
    );
}

/// The highlight index must always sit inside the popup window slice.
fn assert_window_contains_idx(app: &App) {
    let max = super::COMPLETION_POPUP_MAX;
    assert!(
        app.completion_idx >= app.completion_scroll
            && app.completion_idx < app.completion_scroll + max,
        "idx {} must stay inside [{}, {})",
        app.completion_idx,
        app.completion_scroll,
        app.completion_scroll + max
    );
}

/// Popup auto-paging (issue #46): with more matches than
/// `COMPLETION_POPUP_MAX` rows, Down past the window bottom slides the
/// window down so the highlight stays rendered inside the popup; Up slides
/// it back; refreshing the matches resets the window to the top.
#[tokio::test]
async fn completion_popup_pages_past_window_and_scrolls_back_up() {
    let (mut app, _rx) = test_app().await;
    let max = super::COMPLETION_POPUP_MAX;
    // 25 fake matches — far more than the 8-row popup window.
    let n = 25;
    app.completions = (0..n).map(|i| format!("/cmd{i:02}")).collect();
    app.completion_idx = 0;
    app.completion_scroll = 0;

    let backend = TestBackend::new(60, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    assert_highlight_on_popup(&app, terminal.backend().buffer(), "/cmd00");

    // Down past the 8th row: the highlight reaches item 8 (the 9th match)
    // and the window slides down one item to keep it visible.
    for _ in 0..8 {
        app.completion_next();
        assert_window_contains_idx(&app);
    }
    assert_eq!(app.completion_idx, 8);
    assert_eq!(app.completion_scroll, 1);
    terminal.draw(|f| app.render(f)).unwrap();
    assert_highlight_on_popup(&app, terminal.backend().buffer(), "/cmd08");

    // Down to the last match: the window tracks the highlight, so item 24
    // renders on the window's bottom row.
    while app.completion_idx < n - 1 {
        app.completion_next();
        assert_window_contains_idx(&app);
    }
    assert_eq!(app.completion_idx, n - 1);
    assert_eq!(app.completion_scroll, n - max);
    terminal.draw(|f| app.render(f)).unwrap();
    assert_highlight_on_popup(&app, terminal.backend().buffer(), "/cmd24");

    // Down wraps to the top: window back to item 0.
    app.completion_next();
    assert_eq!(app.completion_idx, 0);
    assert_eq!(app.completion_scroll, 0);
    terminal.draw(|f| app.render(f)).unwrap();
    assert_highlight_on_popup(&app, terminal.backend().buffer(), "/cmd00");

    // Up wraps to the tail: the window jumps to the last page, then Up
    // across the top edge slides it back down as the highlight rises.
    app.completion_prev();
    assert_eq!(app.completion_idx, n - 1);
    assert_eq!(app.completion_scroll, n - max);
    for _ in 0..8 {
        app.completion_prev();
        assert_window_contains_idx(&app);
    }
    assert_eq!(app.completion_idx, 16);
    assert_eq!(app.completion_scroll, 16);
    terminal.draw(|f| app.render(f)).unwrap();
    assert_highlight_on_popup(&app, terminal.backend().buffer(), "/cmd16");

    // Refreshing the matches (any input edit) resets the window to the top.
    app.set_input("/");
    assert_eq!(app.completion_scroll, 0, "refresh must reset the window");
    assert_eq!(app.completion_idx, 0);
    assert_window_contains_idx(&app);
}

/// Catalog entries (issue #47): every ENABLED skill appears as
/// `skill::<name>` and every MCP tool as `mcp:<tool>` with verbatim names,
/// stored with the `/` prefix like every other completion. Existing
/// `/shortcut` entries and all other commands stay in the list.
#[test]
fn collect_slash_commands_appends_skill_and_mcp_catalog_entries() {
    // Arrange
    let registry = crate::local_commands::local_registry();
    let skills = vec![
        WireSkillSnapshot {
            name: "code-review".into(),
            source: "user".into(),
            file_path: "/skills/code-review".into(),
            enabled: true,
        },
        WireSkillSnapshot {
            name: "secrets-check".into(),
            source: "builtin".into(),
            file_path: "/skills/secrets".into(),
            enabled: false,
        },
    ];
    let mcp_tools = vec!["fetch_url".to_string(), "Server_Uppercase_Tool".to_string()];

    // Act
    let commands = collect_slash_commands(&registry, &skills, &[], &mcp_tools);

    // Assert
    assert!(
        commands.contains(&"/skill::code-review".to_string()),
        "enabled skills must appear behind the skill:: prefix"
    );
    assert!(
        !commands.contains(&"/skill::secrets-check".to_string()),
        "disabled skills must not appear in the catalog"
    );
    assert!(commands.contains(&"/mcp:fetch_url".to_string()));
    assert!(
        commands.contains(&"/mcp:Server_Uppercase_Tool".to_string()),
        "MCP tool names must stay verbatim, never rewritten"
    );
    // Existing entries are preserved alongside the new catalog entries.
    assert!(commands.contains(&"/help".to_string()));
    assert!(commands.contains(&"/code-review".to_string()));
}

/// Popup prefix filtering with catalog entries (issue #47): typing
/// `/skill::` narrows the popup to skill catalog entries only and `/mcp:`
/// to MCP tools — plain commands and `/shortcut` entries disappear.
#[tokio::test]
async fn slash_popup_filters_skill_and_mcp_catalogs_by_prefix() {
    let (mut app, _rx) = test_app().await;
    // Arrange: seed the snapshot sidebar the popup completer reads from.
    app.latest.sidebar.skills.items = vec![
        WireSkillSnapshot {
            name: "code-review".into(),
            source: "user".into(),
            file_path: "/skills/code-review".into(),
            enabled: true,
        },
        WireSkillSnapshot {
            name: "secrets-check".into(),
            source: "builtin".into(),
            file_path: "/skills/secrets".into(),
            enabled: false,
        },
    ];
    app.latest.sidebar.mcp.tool_names = vec!["fetch_url".into()];

    // Act: a bare slash lists the catalog entries among the commands.
    app.set_input("/");
    assert!(
        app.completions.contains(&"/skill::code-review".to_string()),
        "bare slash must list enabled skill catalog entries"
    );
    assert!(
        app.completions.contains(&"/mcp:fetch_url".to_string()),
        "bare slash must list MCP catalog entries"
    );

    // Act: the skill catalog prefix filters everything else out.
    app.set_input("/skill::");
    assert!(!app.completions.is_empty());
    assert!(
        app.completions.iter().all(|c| c.starts_with("/skill::")),
        "skill prefix must leave only skill entries, got {:?}",
        app.completions
    );
    assert!(app.completions.contains(&"/skill::code-review".to_string()));
    assert!(
        !app.completions
            .contains(&"/skill::secrets-check".to_string())
    );

    // Act: the mcp catalog prefix filters to MCP tools only.
    app.set_input("/mcp:");
    assert!(!app.completions.is_empty());
    assert!(
        app.completions.iter().all(|c| c.starts_with("/mcp:")),
        "mcp prefix must leave only mcp entries, got {:?}",
        app.completions
    );
    assert!(app.completions.contains(&"/mcp:fetch_url".to_string()));
}

#[tokio::test]
async fn paste_object_is_atomic_and_keeps_full_text_for_send() {
    let (mut app, _rx) = test_app().await;
    // More than 3 lines → paste object.
    let long = ["alpha", "beta", "gamma", "delta"].join("\n");
    app.insert_paste_text(long.clone());
    assert_eq!(app.input.text(), long, "buffer keeps the full pasted text");
    assert_eq!(app.input.elements().len(), 1);
    let display = app.input.elements()[0].display.clone().unwrap();
    let chip: String = display.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(chip, format!("[ paste {} chars ]", long.chars().count()));
    // The chip renders as one visual line even though the buffer is multi-line.
    assert_eq!(app.input_display_lines(), 1);
    // Backspace at the object's end deletes the whole object.
    app.input.delete_backward(1);
    assert_eq!(app.input.text(), "");
    assert!(app.input.elements().is_empty());
    // Up to 3 lines stay plain text (no object chip).
    let short = "one\ntwo\nthree";
    app.insert_paste_text(short.into());
    assert_eq!(app.input.text(), short);
    assert!(app.input.elements().is_empty());
}

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

/// Composer soft-wrap (issue #40): a single logical line that overflows the
/// input box's content width grows the composer via the textarea's own wrap
/// measurement instead of clipping.
#[tokio::test]
async fn composer_rows_wraps_wide_single_line_input() {
    let (mut app, _rx) = test_app().await;
    app.set_input(&"x".repeat(200));

    // Width 60 → content width 55 (chrome pad 2+1 + ❯ 2): the 200-column
    // draft wraps into 4 visual rows.
    let rows = app.composer_rows(60);
    assert!(
        (2..=6).contains(&rows),
        "200 chars at width 60 must wrap into 2..=6 rows, got {rows}"
    );
    assert_eq!(rows, 4, "ceil(200 / 55) = 4 wrapped rows");

    // Wrapping is visual only: the draft stays a single logical line, so
    // history navigation / slash completion / Enter semantics are intact.
    assert!(
        app.input_is_single_line(),
        "wrapping must not turn the draft multi-line"
    );
}

/// Composer soft-wrap cap (issue #40): a draft far taller than
/// `MAX_INPUT_ROWS` re-measures with the scrollbar column reserved
/// and still clamps at the cap.
#[tokio::test]
async fn composer_rows_caps_very_long_input_at_max() {
    let (mut app, _rx) = test_app().await;
    app.set_input(&"x".repeat(2000));
    assert_eq!(app.composer_rows(60), super::MAX_INPUT_ROWS as u16);
}
