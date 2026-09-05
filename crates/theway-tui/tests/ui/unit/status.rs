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
        total_input_tokens: 57_100,
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
async fn busy_stats_follow_working_instead_of_right_aligning() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.usage = WireContextUsage {
        total_input_tokens: 57_100,
        output_tokens: 1_200,
        ..Default::default()
    };
    app.apply_snapshot(status);
    app.busy = true;
    let now = std::time::Instant::now();
    app.cps_meter.record_at(now - Duration::from_millis(500), 0);
    app.cps_meter.record_at(now, 500);

    let backend = TestBackend::new(100, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let row = text
        .lines()
        .find(|line| line.contains("working"))
        .expect("busy status row");
    let working = row.find("working").unwrap();
    let throughput = row.find("t/s").expect("busy throughput");
    assert!(
        throughput - working < 30,
        "throughput must follow working without a flexible gap: {row}"
    );
    assert!(row.contains("out: 1.2k"), "output counter missing: {row}");
    assert!(
        row.chars().count() < 80,
        "left cluster must not expand to the 100-column right edge: {row}"
    );
}

#[tokio::test]
async fn narrow_busy_status_truncates_the_left_cluster_safely() {
    let (mut app, _rx) = test_app().await;
    app.busy = true;
    let backend = TestBackend::new(24, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("working"), "busy label missing:\n{text}");
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

#[tokio::test]
async fn esc_while_busy_aborts_even_with_completion_popup() {
    let (mut app, mut rx) = test_app().await;
    app.busy = true;
    app.set_input("/");
    assert!(!app.completions.is_empty());

    let mut terminal = terminal_placeholder();
    app.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
        &mut terminal,
    )
    .await
    .unwrap();

    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no cancel command")
        .unwrap();
    assert!(matches!(cmd, WireCommand::Abort { .. }));
    assert!(app.completions.is_empty());
}

#[tokio::test]
async fn bare_model_slash_opens_picker_and_switches_model() {
    let (mut app, rx) = test_app().await;
    let (_drain, seen) = drain_commands(rx);
    app.dispatch_slash("/model", &mut terminal_placeholder()).await;
    assert!(app.model_picker.is_some());

    // Enter descends into anthropic; Enter again into thinking intensity;
    // Enter a third time selects the first model at the first thinking level.
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
    assert!(app.handle_model_picker_key(&enter).await);
    assert!(app.handle_model_picker_key(&enter).await);
    assert!(app.handle_model_picker_key(&enter).await);

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["SetModel(anthropic:claude-x)", "SetThinking(off)"]
    );
    assert!(app.model_picker.is_none());
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

/// Skill entries (issue #110): each ENABLED skill appears exactly once.
/// The dispatchable `/<shortcut>` is canonical when unique and non-colliding;
/// `/skill::<name>` is the fallback for command collisions or ambiguous
/// names. MCP tools still appear verbatim behind `mcp:`.
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
        WireSkillSnapshot {
            name: "help".into(),
            source: "user".into(),
            file_path: "/skills/help".into(),
            enabled: true,
        },
        WireSkillSnapshot {
            name: "group/a".into(),
            source: "user".into(),
            file_path: "/skills/group/a".into(),
            enabled: true,
        },
        WireSkillSnapshot {
            name: "group/b".into(),
            source: "user".into(),
            file_path: "/skills/group/b".into(),
            enabled: true,
        },
    ];
    let mcp_tools = vec!["fetch_url".to_string(), "Server_Uppercase_Tool".to_string()];

    // Act
    let commands = collect_slash_commands(&registry, &skills, &[], &mcp_tools);

    // Assert: one entry per skill.
    assert!(commands.contains(&"/code-review".to_string()));
    assert!(
        !commands.contains(&"/skill::code-review".to_string()),
        "unique shortcuts must not duplicate as skill:: entries"
    );
    assert!(
        !commands.contains(&"/secrets-check".to_string()),
        "disabled skills must not appear"
    );
    // `/help` collides with the local command, so the exact-name fallback is used.
    assert!(commands.contains(&"/skill::help".to_string()));
    // Two skills share the `group` first segment: both fall back to exact names.
    assert!(!commands.contains(&"/group".to_string()));
    assert!(commands.contains(&"/skill::group/a".to_string()));
    assert!(commands.contains(&"/skill::group/b".to_string()));
    assert!(commands.contains(&"/mcp:fetch_url".to_string()));
    assert!(
        commands.contains(&"/mcp:Server_Uppercase_Tool".to_string()),
        "MCP tool names must stay verbatim, never rewritten"
    );
    // Existing entries are preserved alongside the new catalog entries.
    assert!(commands.contains(&"/help".to_string()));
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
        WireSkillSnapshot {
            name: "help".into(),
            source: "user".into(),
            file_path: "/skills/help".into(),
            enabled: true,
        },
    ];
    app.latest.sidebar.mcp.tool_names = vec!["fetch_url".into()];

    // Act: a bare slash lists one entry per skill: the unique shortcut for
    // `code-review` and the exact-name fallback for the colliding `help`.
    app.set_input("/");
    assert!(
        app.completions.contains(&"/code-review".to_string()),
        "bare slash must list unique skill shortcuts"
    );
    assert!(
        !app.completions.contains(&"/skill::code-review".to_string()),
        "unique shortcuts must not duplicate as skill:: entries"
    );
    assert!(
        app.completions.contains(&"/skill::help".to_string()),
        "colliding skills must appear behind skill::"
    );
    assert!(
        app.completions.contains(&"/mcp:fetch_url".to_string()),
        "bare slash must list MCP catalog entries"
    );

    // Act: the skill catalog prefix filters to exact-name fallback entries.
    app.set_input("/skill::");
    assert!(!app.completions.is_empty());
    assert!(
        app.completions.iter().all(|c| c.starts_with("/skill::")),
        "skill prefix must leave only skill entries, got {:?}",
        app.completions
    );
    assert!(app.completions.contains(&"/skill::help".to_string()));
    assert!(!app.completions.contains(&"/skill::code-review".to_string()));
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

#[tokio::test]
async fn composer_info_line_shows_current_working_directory() {
    let (mut app, _rx) = test_app().await;
    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("/tmp/theway"), "composer cwd missing:\n{text}");
}

// ── session-cumulative KV cache metrics (busy band) ──────────────────────

#[tokio::test]
async fn busy_stats_line_shows_session_cache_metrics_from_session_usage() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.busy = true;
    status.session_usage = WireContextUsage {
        cached_tokens: 800,
        new_tokens: 400,
        total_input_tokens: 1_200,
        output_tokens: 340,
        cache_write_tokens: 50,
        provider_cache_hit_rate: Some(800.0 / 1_200.0),
        prefix_cache_hit_rate: None,
        prefix_hit_tokens: 0,
        context_window: 200_000,
    };
    app.apply_snapshot(status);
    app.busy = true;

    let backend = TestBackend::new(120, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let row = text
        .lines()
        .find(|line| line.contains("working"))
        .expect("busy status row");

    // Total input, output, and the provider cache hit rate.
    assert!(row.contains("in: 1.2k"), "total input missing: {row}");
    assert!(row.contains("out: 340"), "output missing: {row}");
    assert!(row.contains("cache 66.7%"), "provider cache hit rate missing: {row}");
    // The streamlined status line drops the cached/new/prefix breakdown.
    assert!(!row.contains("cached:"), "cached tokens must not appear: {row}");
    assert!(!row.contains("new:"), "non-cached input must not appear: {row}");
    assert!(!row.contains("prefix"), "prefix hit rate must not appear: {row}");
    assert!(!row.contains("char/s"), "char/s must not appear: {row}");

    // The status line is a token/cache display, not a cost display: monetary
    // values must never appear.
    assert!(!row.contains('$'), "monetary values must not appear: {row}");
}
