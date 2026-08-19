#[test]
fn extract_text_truncates_boundary_rows_and_joins_with_newlines() {
    let lines = vec![
        Line::from("hello world"),
        Line::from("middle row"),
        Line::from("tail"),
    ];
    let sel = FeedSelection {
        anchor: (0, 6),
        head: (2, 2),
    };
    assert_eq!(
        selection::extract_text(&lines, &sel),
        "world\nmiddle row\nta"
    );

    // Reversed direction extracts the same normalized text.
    let rev = FeedSelection {
        anchor: (2, 2),
        head: (0, 6),
    };
    assert_eq!(
        selection::extract_text(&lines, &rev),
        "world\nmiddle row\nta"
    );

    // Wide chars cut on display columns, not bytes: "中文文本" is 8 columns.
    let wide = vec![Line::from("中文文本")];
    let sel = FeedSelection {
        anchor: (0, 2),
        head: (0, 6),
    };
    assert_eq!(selection::extract_text(&wide, &sel), "文文");

    // Trailing filler spans (band padding) never leak into the copy.
    let padded = Line::from(vec![
        Span::styled("abc", Style::default()),
        Span::styled("     ", Style::default()),
    ]);
    let sel = FeedSelection {
        anchor: (0, 0),
        head: (0, 50),
    };
    assert_eq!(selection::extract_text(&[padded], &sel), "abc");
}

#[test]
fn highlight_cols_paints_only_the_selected_column_range() {
    let line = Line::from(vec![
        Span::styled("abcd", Style::default().fg(Color::Red)),
        Span::styled("efgh", Style::default().fg(Color::Green)),
    ]);
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 2));
    selection::highlight_cols(&mut buf, 0, 0, &line, 2, 6);

    let bg = selection::BAND_STYLE.bg.unwrap();
    for x in 0..20u16 {
        let cell = &buf[(x, 0)];
        if (2..6).contains(&x) {
            assert_eq!(cell.bg, bg, "column {x} must carry the selection bg");
        } else {
            assert_ne!(cell.bg, bg, "column {x} must keep its own style");
        }
    }
    // Original colors survive under the selection bg.
    assert_eq!(buf[(3, 0)].fg, Color::Red);
    assert_eq!(buf[(5, 0)].fg, Color::Green);
    // The second row is untouched.
    assert_ne!(buf[(3, 1)].bg, bg);
}

fn dag_run(kind: &str) -> theway_transport::wire::WireDagRunSnapshot {
    theway_transport::wire::WireDagRunSnapshot {
        id: "dag-1".into(),
        name: "demo".into(),
        kind: kind.into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 4,
        direction: "TD".into(),
        created_at: 0,
        completed_at: None,
        error: None,
        nodes: Vec::new(),
    }
}

#[test]
fn feature_labels_empty_without_sources() {
    assert!(super::feature_labels(&[]).is_empty());
    // Non-dag-kind runs (e.g. goal) do not activate a composer label.
    assert!(super::feature_labels(&[dag_run("goal")]).is_empty());
}

#[test]
fn feature_labels_derives_graph_engine_from_dag_run() {
    let labels = super::feature_labels(&[dag_run("dag")]);
    assert_eq!(labels, vec!["graph engine".to_string()]);
}

#[tokio::test]
async fn chrome_info_line_drops_working_and_multiline() {
    let (mut app, _rx) = test_app().await;
    // Busy (the old "working" flag case) with a multiline draft (the old
    // "multiline" indicator case): neither may appear on the info line.
    let mut status = fixture_status(Vec::new());
    status.busy = true;
    app.apply_snapshot(status);
    app.set_input("line one\nline two");
    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let lines: Vec<&str> = text.lines().collect();
    let info_row = lines
        .iter()
        .find(|l| l.contains('╰'))
        .unwrap_or_else(|| panic!("composer info line missing:\n{text}"));
    assert!(info_row.contains("provider:model"), "info row: {info_row}");
    assert!(!info_row.contains("working"), "info row: {info_row}");
    assert!(!info_row.contains("multiline"), "info row: {info_row}");
    // The busy band above still carries the working label.
    assert!(
        text.contains("working"),
        "busy band lost its label:\n{text}"
    );
}

#[tokio::test]
async fn chrome_top_divider_shows_feature_labels() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.dags = vec![dag_run("dag")];
    app.apply_snapshot(status);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let lines: Vec<&str> = text.lines().collect();
    let divider = lines
        .iter()
        .find(|l| l.contains('╭'))
        .unwrap_or_else(|| panic!("composer top divider missing:\n{text}"));
    assert!(
        divider.contains("graph engine") && !divider.contains("goal"),
        "divider row: {divider}"
    );
}

#[tokio::test]
async fn chrome_top_divider_blank_without_features() {
    let (mut app, _rx) = test_app().await;
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let lines: Vec<&str> = text.lines().collect();
    let divider = lines
        .iter()
        .find(|l| l.contains('╭'))
        .unwrap_or_else(|| panic!("composer top divider missing:\n{text}"));
    assert!(
        divider.chars().all(|c| c == '╭' || c == '─' || c == '╮'),
        "divider row must stay bare without features: {divider}"
    );
}

// ── theme (issues #43 + #49) ──────────────────────────────────────────────

/// Feed row whose rendered text contains `needle` (theme block tests).
fn feed_row_containing(buf: &Buffer, feed: Rect, needle: &str) -> u16 {
    (feed.y..feed.y.saturating_add(feed.height))
        .find(|y| {
            let row: String = (feed.x..feed.x.saturating_add(feed.width))
                .map(|x| buf[(x, *y)].symbol())
                .collect();
            row.contains(needle)
        })
        .unwrap_or_else(|| panic!("no feed row contains {needle:?}"))
}

/// Assert a feed row carries `bg` across the FULL feed width, its content
/// contains `needle`, and the content ends `right_pad` columns before the
/// right edge (align=right block layout, issue #49).
fn assert_block_row(buf: &Buffer, feed: Rect, y: u16, bg: Color, right_pad: u16, needle: &str) {
    let mut text = String::new();
    let mut cols: Vec<&str> = Vec::with_capacity(feed.width as usize);
    for x in feed.x..feed.x.saturating_add(feed.width) {
        let cell = &buf[(x, y)];
        assert_eq!(
            cell.bg, bg,
            "cell ({x},{y}) must carry the block background"
        );
        text.push_str(cell.symbol());
        cols.push(cell.symbol());
    }
    assert!(text.contains(needle), "content missing from row: {text}");
    let last = cols
        .iter()
        .rposition(|s| *s != " ")
        .unwrap_or_else(|| panic!("row has no visible content: {text}"));
    assert_eq!(
        last,
        feed.width as usize - 1 - right_pad as usize,
        "content must end {right_pad} columns before the right edge: {text}"
    );
    assert!(
        cols[last + 1..].iter().all(|s| *s == " "),
        "trailing padding must be background spaces: {text}"
    );
}

/// Custom theme (issue #49): tool / tool-result / thinking blocks paint their
/// background across the FULL block width with the configured padding
/// columns, content right-aligned; an empty result line renders as pure
/// background.
#[tokio::test]
async fn custom_theme_paints_tool_and_thinking_blocks() {
    let (mut app, _rx) = test_app().await;
    let running_bg = Color::Rgb(50, 60, 70);
    let success_bg = Color::Rgb(60, 70, 80);
    let thinking_bg = Color::Rgb(70, 80, 90);
    let mut theme = Theme::default();
    theme.tool_running_bg = Some(running_bg);
    theme.tool_success_bg = Some(success_bg);
    theme.tool.padding = 2;
    theme.tool.align = BlockAlign::Right;
    theme.thinking_bg = Some(thinking_bg);
    theme.thinking.padding = 1;
    theme.thinking.align = BlockAlign::Right;
    app.theme = theme;
    app.tools_expanded = true;
    let status = fixture_status(vec![
        WireFeedBlock::Tool {
            name: "read".into(),
            args: "(path=\"x\")".into(),
            timestamp: None,
        },
        WireFeedBlock::ToolResult {
            lines: vec!["first".into(), String::new(), "third".into()],
            is_error: false,
            timestamp: None,
        },
        WireFeedBlock::Thinking {
            text: "pondering the design".into(),
            timestamp: None,
        },
    ]);
    app.apply_snapshot(status);

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let feed = app.last_feed_area.unwrap();
    let text = buffer_text(buf);

    // Tool call row: running bg, padding 2, right-aligned.
    let tool_row = feed_row_containing(buf, feed, "⏵ read (path=\"x\")");
    assert_block_row(buf, feed, tool_row, running_bg, 2, "⏵ read (path=\"x\")");

    // Tool result rows: success bg + same block layout.
    let result_row = feed_row_containing(buf, feed, "first");
    assert_block_row(buf, feed, result_row, success_bg, 2, "first");
    // Empty result line → pure background row (all cells bg, all spaces).
    let empty_row = (feed.y..feed.y.saturating_add(feed.height)).find(|y| {
        (feed.x..feed.x.saturating_add(feed.width))
            .all(|x| buf[(x, *y)].symbol() == " " && buf[(x, *y)].bg == success_bg)
    });
    assert!(
        empty_row.is_some(),
        "empty result row must render as pure background"
    );

    // Thinking stats line and body row: thinking bg, padding 1, right-aligned.
    let stats_row = feed_row_containing(buf, feed, "c/s:");
    assert_block_row(buf, feed, stats_row, thinking_bg, 1, "⏵ thinking");
    let body_row = feed_row_containing(buf, feed, "pondering the design");
    assert_block_row(buf, feed, body_row, thinking_bg, 1, "pondering the design");
    assert!(text.contains("⏵ thinking"), "{text}");
}

/// Custom composer style (issue #49): the prompt chrome reads
/// `[composer]` colors from the theme — border, prefix, background and the
/// blended info-line caption all change.
#[tokio::test]
async fn custom_theme_recolors_composer_chrome() {
    let (mut app, _rx) = test_app().await;
    let border = Color::Rgb(200, 100, 50);
    let prefix = Color::Rgb(50, 200, 100);
    let bg = Color::Rgb(10, 20, 30);
    let info = Color::Rgb(240, 240, 200);
    let mut theme = Theme::default();
    theme.composer.border_focused = border;
    theme.composer.prefix = prefix;
    theme.composer.bg = bg;
    theme.composer.info_text = info;
    app.theme = theme;

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let input = app.last_input_area.unwrap();

    // Top divider corner: focused border color over the prompt background.
    let corner = &buf[(input.x, input.y)];
    assert_eq!(corner.fg, border, "divider border color");
    assert_eq!(corner.bg, bg, "prompt background on the divider");
    // Filled prompt surface left of the ❯ prefix.
    assert_eq!(buf[(input.x + 1, input.y + 1)].bg, bg);
    // ❯ prefix uses the theme prefix color.
    assert_eq!(buf[(input.x + 2, input.y + 1)].fg, prefix);
    // Info-line caption: info_text blended onto bg at 0.6 (focused), same
    // blend the chrome applies.
    let expected = theway_pager_render::color::blend_color(bg, info, 0.6)
        .unwrap_or(super::prompt_chrome::GRAY);
    let info_row = input.y + input.height - 1;
    let p_x = (input.x..input.x.saturating_add(input.width))
        .find(|x| buf[(*x, info_row)].symbol() == "p")
        .expect("model name missing from the info line");
    assert_eq!(buf[(p_x, info_row)].fg, expected, "info caption color");
}

/// Default theme: with no theme.toml the tool/thinking blocks keep the
/// pre-theme visuals — no background, no padding columns (issue #49).
#[tokio::test]
async fn default_theme_keeps_feed_blocks_unpainted() {
    let (mut app, _rx) = test_app().await;
    let status = fixture_status(vec![
        WireFeedBlock::Tool {
            name: "read".into(),
            args: "(path=\"x\")".into(),
            timestamp: None,
        },
        WireFeedBlock::Thinking {
            text: "pondering".into(),
            timestamp: None,
        },
    ]);
    app.apply_snapshot(status);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let feed = app.last_feed_area.unwrap();
    let tool_row = feed_row_containing(buf, feed, "⏵ read");
    let tool_cell = &buf[(feed.x, tool_row)];
    assert_eq!(tool_cell.symbol(), "⏵", "tool row must stay flush-left");
    assert_eq!(tool_cell.bg, Color::Reset, "no background without a theme");
    let stats_row = feed_row_containing(buf, feed, "⏵ thinking");
    let stats_cell = &buf[(feed.x, stats_row)];
    assert_eq!(
        stats_cell.symbol(),
        "⏵",
        "thinking row must stay flush-left"
    );
    assert_eq!(stats_cell.bg, Color::Reset, "no background without a theme");
}

fn dag_node(id: &str, status: &str) -> theway_transport::wire::WireDagNodeSnapshot {
    theway_transport::wire::WireDagNodeSnapshot {
        id: id.into(),
        agent: "executor-coder".into(),
        status: status.into(),
        depends_on: Vec::new(),
        job_id: None,
        attempt: 1,
        started_at: None,
        completed_at: None,
        error: None,
        input_tokens: None,
        output_tokens: None,
        result: None,
        output_tail: None,
        live_preview: None,
    }
}

#[tokio::test]
async fn dag_band_renders_between_feed_and_busy() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.busy = true;
    let mut run = dag_run("dag");
    let mut failed = dag_node("2-impl", "failed");
    failed.error = Some("compile error".into());
    run.nodes = vec![
        dag_node("1-explore", "succeeded"),
        failed,
        dag_node("3-verify", "running"),
        dag_node("4-ship", "pending"),
    ];
    status.dags = vec![run];
    app.apply_snapshot(status);
    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let lines: Vec<&str> = text.lines().collect();
    let header = lines
        .iter()
        .position(|l| l.contains("dag-1 · demo"))
        .unwrap_or_else(|| panic!("dag band header missing:\n{text}"));
    assert!(lines[header].contains("1/4"), "header: {}", lines[header]);
    assert!(lines[header].contains("c/s"), "header: {}", lines[header]);
    // A running node renders the braille mini spinner on the header.
    assert!(
        lines[header]
            .chars()
            .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
        "header: {}",
        lines[header]
    );
    // The node row with state glyphs and the error summary follows.
    let node_row = lines[header + 1];
    for needle in [
        "✓ 1-explore",
        "✗ 2-impl compile error",
        "▶ 3-verify",
        "· 4-ship",
    ] {
        assert!(node_row.contains(needle), "node row: {node_row}");
    }
    // The band sits between the feed and the busy band (above "working").
    let working = lines
        .iter()
        .position(|l| l.contains("working"))
        .unwrap_or_else(|| panic!("busy band missing:\n{text}"));
    assert!(header < working, "band must sit above the busy band");
}

#[tokio::test]
async fn dag_band_caps_at_two_runs_with_more_line() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    let mut runs = Vec::new();
    for n in 1..=3 {
        let mut run = dag_run("dag");
        run.id = format!("dag-{n}");
        runs.push(run);
    }
    status.dags = runs;
    app.apply_snapshot(status);
    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("dag-1 · demo"), "{text}");
    assert!(text.contains("dag-2 · demo"), "{text}");
    assert!(text.contains("… 1 more"), "{text}");
    assert!(!text.contains("dag-3"), "{text}");
}
