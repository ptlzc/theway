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

// ── busy snake loader (issue #42) ─────────────────────────────────────────

/// Head trajectory: the triangular wave follows the row-snake order through
/// the 3×3 grid and reverses at both ends.
#[test]
fn snake_head_bounces_through_the_three_by_three_track() {
    let expected = [0usize, 1, 2, 5, 4, 3, 6, 7, 8, 7, 6, 3, 4, 5, 2, 1];
    for (step, &want) in expected.iter().enumerate() {
        assert_eq!(snake_loader::head_pos(step as u64), want, "step {step}");
    }
    // The wave repeats every 16 steps and never leaves the track.
    for step in 0..=64u64 {
        let pos = snake_loader::head_pos(step);
        assert!(pos < 9, "step {step}: head left the track ({pos})");
        assert_eq!(
            snake_loader::head_pos(step + 16),
            pos,
            "step {step}: wave must repeat every 16 steps"
        );
    }
}

/// Tail follow: segment `i` sits where the head was `i` steps ago, and
/// at a reversal the tail flips to the far side of the motion direction.
#[test]
fn snake_tail_follows_head_history_and_flips_at_reversal() {
    // At the final grid cell, the tail follows the preceding bottom-row dots.
    assert_eq!(snake_loader::segment_pos(8, 0), Some(8));
    assert_eq!(snake_loader::segment_pos(8, 1), Some(7));
    assert_eq!(snake_loader::segment_pos(8, 2), Some(6));
    // Step 9 reverses: head returns to 7 and the tail remains at endpoint 8.
    assert_eq!(snake_loader::segment_pos(9, 0), Some(7));
    assert_eq!(snake_loader::segment_pos(9, 1), Some(8));
    // History predating the wave start is out of range: the segment has
    // no track cell and renders dim.
    assert_eq!(snake_loader::segment_pos(0, 0), Some(0));
    assert_eq!(snake_loader::segment_pos(0, 1), None);
    assert_eq!(snake_loader::segment_pos(2, 3), None);
}

/// Rainbow gradient: every step rotates the hue wheel by 15° and each
/// trail segment adds a 40° offset, all through the shared HSV→RGB
/// conversion.
#[test]
fn snake_rainbow_hues_advance_per_step_and_segment() {
    // The head is fully lit (value 1.0), so its color follows the
    // step*15° hue exactly.
    for step in [0u64, 1, 3, 7] {
        let frame = snake_loader::snake_frame(step, 0.0);
        let pos = snake_loader::head_pos(step);
        let (r, g, b) = super::pixel_loader::hsv_to_rgb((step as f32 * 15.0) % 360.0, 0.85, 1.0);
        assert_eq!(frame.cells[pos].fg, Color::Rgb(r, g, b), "step {step}");
    }
    // Within one frame the trail steps 40° per segment: adjacent
    // segments carry different colors.
    let frame = snake_loader::snake_frame(16, 0.0);
    let colors: Vec<Color> = frame
        .cells
        .iter()
        .filter(|c| c.lit > 0.0)
        .map(|c| c.fg)
        .collect();
    assert_eq!(colors.len(), 2, "idle trail lights head + one segment");
    assert_ne!(colors[0], colors[1], "trail hues must differ by segment");
    // Hue rotates with step even when the head revisits the same cell
    // (bounce positions repeat every 16 steps, colors every 24).
    let a = snake_loader::snake_frame(0, 0.0).cells[0].fg;
    let b = snake_loader::snake_frame(16, 0.0).cells[0].fg;
    assert_ne!(a, b);
}

/// Trail length: 2 segments at rest growing with throughput, capped at
/// 5; segments whose history predates the wave start stay dim.
#[test]
fn snake_trail_grows_from_two_to_five_segments() {
    assert_eq!(snake_loader::trail_len(0.0), 2.0);
    assert_eq!(snake_loader::trail_len(1e9), 5.0);
    // A full-cycle step has enough history to fill the complete trail.
    let idle = snake_loader::snake_frame(16, 0.0);
    assert_eq!(
        idle.cells.iter().filter(|c| c.lit > 0.0).count(),
        2,
        "idle trail must light 2 cells"
    );
    let fast = snake_loader::snake_frame(16, 1e9);
    assert_eq!(
        fast.cells.iter().filter(|c| c.lit > 0.0).count(),
        5,
        "speed-cap trail must light 5 cells"
    );
    // History predating the wave start renders dim: only the head lit.
    let early = snake_loader::snake_frame(0, 1e9);
    assert_eq!(early.cells.iter().filter(|c| c.lit > 0.0).count(), 1);
    assert_eq!(early.cells[0].lit, 1.0);
}

/// Track stability: all nine cells render every frame — lit cells carry
/// the rainbow body and unlit ones stay as dim dots.
#[test]
fn snake_track_always_shows_all_nine_round_dots() {
    for step in [0u64, 4, 8, 9, 15, 23, 100] {
        for cps in [0.0, 500.0, 1e9] {
            let frame = snake_loader::snake_frame(step, cps);
            assert_eq!(frame.cells.len(), 9, "step {step}");
            for (i, cell) in frame.cells.iter().enumerate() {
                assert_eq!(cell.glyph, '•', "step {step} cell {i}");
                assert_eq!(cell.bg, Color::Reset, "step {step} cell {i}");
                if cell.lit == 0.0 {
                    assert_eq!(cell.fg, Color::DarkGray, "step {step} cell {i}");
                }
            }
        }
    }
}

#[test]
fn compact_snake_columns_keep_the_brightest_segment_color() {
    for step in [0u64, 4, 8, 9, 15, 23, 100] {
        let frame = snake_loader::snake_frame(step, 60.0);
        let columns = snake_loader::compact_columns(&frame);
        for (column, compact) in columns.iter().enumerate() {
            let brightest = (0..snake_loader::GRID_HEIGHT)
                .map(|row| frame.cells[row * snake_loader::GRID_WIDTH + column])
                .max_by(|left, right| left.lit.total_cmp(&right.lit))
                .unwrap();
            assert_eq!(*compact, brightest, "step {step}, column {column}");
        }
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
    let long = "x".repeat(25);
    app.insert_paste_text(long.clone());
    assert_eq!(app.input.text(), long, "buffer keeps the full pasted text");
    assert_eq!(app.input.elements().len(), 1);
    let display = app.input.elements()[0].display.clone().unwrap();
    let chip: String = display.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(chip, "[ paste 25 chars ]");
    // The chip renders as one visual line even though the buffer is long.
    assert_eq!(app.input_display_lines(), 1);
    // Backspace at the object's end deletes the whole object.
    app.input.delete_backward(1);
    assert_eq!(app.input.text(), "");
    assert!(app.input.elements().is_empty());
    // Short pastes stay plain text.
    app.insert_paste_text("short".into());
    assert_eq!(app.input.text(), "short");
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
