/// Issue #56: `/resume` completes as a TUI-local command (`LOCAL_COMMANDS`)
/// and stays out of the daemon-side command table the client forwards.
#[test]
fn collect_slash_commands_includes_local_resume_command() {
    // Arrange
    let registry = crate::local_commands::local_registry();

    // Act
    let commands = collect_slash_commands(&registry, &[], &[], &[]);

    // Assert
    assert!(
        commands.contains(&"/resume".to_string()),
        "completion list must contain /resume, got: {commands:?}"
    );
    assert!(
        !super::DAEMON_COMMANDS.contains(&"resume"),
        "/resume is TUI-local and must not live in the daemon command table"
    );
}

/// Issue #56: `/help` lists `/resume` in the local command surface.
#[tokio::test]
async fn help_line_lists_resume_command() {
    let (mut app, _rx) = test_app().await;

    // Act
    app.dispatch_slash("/help", &mut terminal_placeholder())
        .await;

    // Assert
    let text = feed_text(&app);
    assert!(
        text.contains("/resume"),
        "help text must list /resume, got: {text}"
    );
}

/// Issue #56: `/resume` opens a popup over `list_sessions` in the daemon's
/// tree order (oldest → newest) with short id + name + busy/graph marks,
/// the current session annotated and pre-selected.
#[tokio::test]
async fn resume_picker_lists_sessions_and_annotates_current() {
    let (mut app, mut rx, _ops) = test_app_with_sessions(&["sess-1", "sess-2"]).await;
    // Arrange: make sess-2 the daemon's current session (drain the command).
    assert!(app.client.switch_session("sess-2").await.unwrap());
    rx.recv().await.expect("switch command");

    // Act
    app.dispatch_slash("/resume", &mut terminal_placeholder())
        .await;

    // Assert: tree order, current annotated + selected, short ids present.
    let picker = app
        .resume_picker
        .as_ref()
        .expect("/resume must open the picker");
    assert_eq!(picker.entries.len(), 2);
    assert_eq!(picker.entries[0].id, "sess-1");
    assert!(!picker.entries[0].current);
    assert_eq!(picker.entries[1].id, "sess-2");
    assert!(picker.entries[1].current);
    assert_eq!(picker.selected, 1, "the current session is pre-selected");
    assert_eq!(picker.entries[1].id_short, "sess-2");

    // Render: popup lists both sessions and the current annotation.
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("resume"), "popup title missing:\n{text}");
    assert!(text.contains("sess-1"), "session row missing:\n{text}");
    assert!(
        text.contains("sess-2 · current"),
        "current row must be annotated, got:\n{text}"
    );
}

/// Issue #56: Enter switches to the highlighted session over the
/// SwitchSession RPC (queued daemon-side — the next snapshot presents the
/// new session) and closes the popup; Esc cancels without sending anything.
#[tokio::test]
async fn resume_picker_enter_switches_session_and_esc_cancels() {
    let (mut app, mut rx, _ops) = test_app_with_sessions(&["sess-1", "sess-2"]).await;
    let mut term = terminal_placeholder();
    app.dispatch_slash("/resume", &mut term).await;
    assert!(app.resume_picker.is_some());

    // Act: Down highlights sess-2, Enter switches to it.
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

    // Assert: the daemon receives SwitchSession{id}; popup closed.
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no switch_session command")
        .unwrap();
    match cmd {
        WireCommand::SwitchSession { id } => assert_eq!(id, "sess-2"),
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(app.resume_picker.is_none(), "Enter must close the picker");

    // Act: reopen and cancel with Esc.
    app.dispatch_slash("/resume", &mut term).await;
    assert!(app.resume_picker.is_some());
    app.handle_event(
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())),
        &mut term,
    )
    .await
    .unwrap();

    // Assert: closed, nothing forwarded.
    assert!(app.resume_picker.is_none(), "Esc must cancel the picker");
    assert!(
        rx.try_recv().is_err(),
        "Esc must not forward any command to the daemon"
    );
}

/// Issue #56: an empty session list prints the system hint instead of
/// opening an empty popup.
#[tokio::test]
async fn resume_picker_empty_list_prints_no_sessions_hint() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&[]).await;

    // Act
    app.dispatch_slash("/resume", &mut terminal_placeholder())
        .await;

    // Assert
    assert!(
        app.resume_picker.is_none(),
        "an empty list must not open the popup"
    );
    let text = feed_text(&app);
    assert!(
        text.contains("no sessions to resume"),
        "feed must print the empty-list hint, got: {text}"
    );
}

/// Issue #56: the popup row label joins short id + name + marks (`busy`,
/// `graphs N (M active)`, `current`) — a bare session renders just the
/// short id.
#[test]
fn resume_picker_label_formats_name_busy_graph_and_current_marks() {
    // Arrange
    let full = super::ResumePickerEntry {
        id: "abc1234567890".into(),
        id_short: "abc1234567890".into(),
        name: "plan".into(),
        busy: true,
        graph_count: 3,
        active_graph_count: 2,
        current: true,
    };

    // Act + Assert
    assert_eq!(
        super::resume_picker_label(&full),
        "abc1234567890 plan · busy · graphs 3 (2 active) · current"
    );
    let inactive_graphs = super::ResumePickerEntry {
        busy: false,
        active_graph_count: 0,
        current: false,
        ..full.clone()
    };
    assert_eq!(
        super::resume_picker_label(&inactive_graphs),
        "abc1234567890 plan · graphs 3"
    );
    let bare = super::ResumePickerEntry {
        name: String::new(),
        busy: false,
        graph_count: 0,
        active_graph_count: 0,
        current: false,
        ..full.clone()
    };
    assert_eq!(super::resume_picker_label(&bare), "abc1234567890");
}

/// Issue #56 busy-switch path: switching queues on the daemon's event loop
/// (a busy turn aborts and the new session lands on the next snapshot) —
/// `apply_snapshot`'s session-id path presents the new session
/// automatically.
#[tokio::test]
async fn apply_snapshot_updates_session_id_when_switch_lands() {
    let (mut app, _rx) = test_app().await;
    assert_eq!(app.session_id, "sess-1");

    // Arrange: the daemon republishes after a queued SwitchSession.
    let mut status = fixture_status(Vec::new());
    status.session_id = "sess-9".into();

    // Act
    app.apply_snapshot(status);

    // Assert: the app follows the snapshot's session id.
    assert_eq!(app.session_id, "sess-9");
    assert_eq!(app.latest.session_id, "sess-9");
}

/// Issue #50 TUI-side reload: a daemon `reload` bumps the runtime revision,
/// so `apply_snapshot` re-reads `~/.theway/theme.toml` into `App.theme`.
#[tokio::test]
async fn apply_snapshot_reloads_theme_when_runtime_revision_changes() {
    let (mut app, _rx) = test_app().await;
    assert_eq!(app.last_runtime_revision, 0);

    // Arrange: pin a sentinel theme so a reload is observable, then publish
    // a snapshot whose sidebar revision moved.
    let sentinel = {
        let mut theme = Theme::default();
        theme.user_text = Color::Red;
        theme
    };
    app.theme = sentinel;
    let mut status = fixture_status(Vec::new());
    status.sidebar.runtime_revision = 7;

    // Act
    app.apply_snapshot(status);

    // Assert: the theme was reloaded from disk and the revision cached.
    assert_ne!(app.theme, sentinel);
    assert_eq!(app.theme, Theme::load());
    assert_eq!(app.last_runtime_revision, 7);
}

/// Same revision → no theme re-read (every snapshot would otherwise reload).
#[tokio::test]
async fn apply_snapshot_keeps_theme_when_revision_unchanged() {
    let (mut app, _rx) = test_app().await;

    // Arrange: pin a sentinel theme; the snapshot carries the cached revision.
    let sentinel = {
        let mut theme = Theme::default();
        theme.user_text = Color::Red;
        theme
    };
    app.theme = sentinel;

    // Act
    app.apply_snapshot(fixture_status(Vec::new()));

    // Assert: no reload happened.
    assert_eq!(app.theme, sentinel);
    assert_eq!(app.last_runtime_revision, 0);
}

fn feed_text(app: &App) -> String {
    crate::feed_render::lines(
        &app.feed,
        100,
        &crate::feed_render::FeedRenderOptions::default(),
    )
    .into_iter()
    .map(|line| {
        line.spans
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>()
    })
    .collect::<Vec<_>>()
    .join("\n")
}

/// Tests drive `App` methods that only borrow a terminal (never draw);
/// `TestBackend` avoids requiring a controlling TTY in CI.
fn terminal_placeholder() -> Terminal<ratatui::backend::TestBackend> {
    Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap()
}

fn assistant_lines(text: &str, width: usize) -> Vec<ratatui::text::Line<'static>> {
    let mut out: Vec<ratatui::text::Line<'static>> = Vec::new();
    crate::feed_render::push_markdown(
        &mut out,
        text,
        "ai ▸ ",
        ratatui::style::Style::default(),
        width,
        theway_markdown::ColorLevel::TrueColor,
    );
    out
}

fn line_text(line: &ratatui::text::Line<'static>) -> String {
    line.spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
}

fn all_text(lines: &[ratatui::text::Line<'static>]) -> String {
    lines.iter().map(line_text).collect::<String>()
}

#[test]
fn markdown_single_tilde_pair_stays_literal() {
    use ratatui::style::Modifier;
    // Single-tilde pairs are demoted to literal `~` by the shared parser
    // options — the renderer must not strike them. `**10%**` inside still
    // renders as a bold span (pretty mode hides the `**` markers).
    let lines = assistant_lines("~**10%** is not struck", 80);
    let text = all_text(&lines);
    assert!(text.contains("~10%"), "{text}");
    let bold: Vec<&str> = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(bold, vec!["10%"], "{lines:#?}");
}

#[test]
fn markdown_fenced_code_renders_verbatim_no_wrap() {
    // A code line longer than the width must stay on one unwrapped line.
    let long_code = "let x = 1; // ".to_string() + &"a".repeat(120);
    let input = format!("before\n```rust\n{long_code}\n```\nafter");
    let lines = assistant_lines(&input, 40);
    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert!(
        rendered
            .iter()
            .any(|l| l.starts_with("let x = 1;") && l.len() >= long_code.len()),
        "{rendered:#?}"
    );
    // Pretty mode hides the fence delimiters entirely.
    assert!(!rendered.iter().any(|l| l == "```rust"), "{rendered:#?}");
    // The rust body carries syntax-highlighted (colored) spans.
    let colored = lines.iter().any(|l| {
        line_text(l).starts_with("let x = 1;") && l.spans.iter().any(|s| s.style.fg.is_some())
    });
    assert!(colored, "{lines:#?}");
    // Surrounding prose still renders and wraps normally.
    assert!(
        rendered.iter().any(|l| l.contains("before")),
        "{rendered:#?}"
    );
    assert!(
        rendered.iter().any(|l| l.contains("after")),
        "{rendered:#?}"
    );
}

#[test]
fn markdown_parity_bold_italic_inline_code_spans() {
    use ratatui::style::Modifier;
    let lines = assistant_lines("**bold** *em* `code`", 80);
    assert_eq!(line_text(&lines[0]), "ai ▸ bold em code");
    let span = |content: &str| {
        lines[0]
            .spans
            .iter()
            .find(|s| s.content == content)
            .unwrap_or_else(|| panic!("span {content:?} missing: {:?}", lines[0].spans))
    };
    assert!(
        span("bold").style.add_modifier.contains(Modifier::BOLD),
        "{:?}",
        lines[0].spans
    );
    assert!(
        span("em").style.add_modifier.contains(Modifier::ITALIC),
        "{:?}",
        lines[0].spans
    );
    // Inline code is styled distinctly (feed style: bold) with hidden backticks.
    assert!(
        span("code").style.add_modifier.contains(Modifier::BOLD),
        "{:?}",
        lines[0].spans
    );
}

#[test]
fn markdown_parity_heading_is_styled() {
    use ratatui::style::Modifier;
    let lines = assistant_lines("# h", 80);
    // Pretty mode hides the `# ` marker; the heading text is styled.
    assert_eq!(line_text(&lines[0]), "ai ▸ h");
    let heading = lines[0]
        .spans
        .iter()
        .find(|s| s.content == "h")
        .expect("heading span missing");
    assert!(
        heading.style.add_modifier.contains(Modifier::BOLD),
        "{:?}",
        lines[0].spans
    );
}

#[test]
fn markdown_parity_table_renders_border_rows() {
    let lines = assistant_lines("| A | B |\n|---|---|\n| 1 | 2 |", 80);
    let text: Vec<String> = lines.iter().map(line_text).collect();
    assert!(
        text.iter().any(|l| l.contains('┌') && l.contains('┐')),
        "{text:#?}"
    );
    assert!(text.iter().any(|l| l.contains("│ A │")), "{text:#?}");
    assert!(text.iter().any(|l| l.contains("│ 1 │")), "{text:#?}");
}

#[test]
fn markdown_parity_link_gets_underline_from_hyperlinks() {
    use ratatui::style::Modifier;
    let lines = assistant_lines("[text](https://example.com) end", 80);
    let underlined: String = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(underlined.contains("text"), "{underlined}");
    assert!(underlined.contains("https://example.com"), "{underlined}");
}

#[test]
fn markdown_parity_fenced_rust_has_colored_spans() {
    let lines = assistant_lines("```rust\nlet x = 1;\n```", 80);
    let code = lines
        .iter()
        .find(|l| line_text(l).contains("let x"))
        .expect("rust code line missing");
    assert!(
        code.spans.iter().any(|s| s.style.fg.is_some()),
        "syntax highlighting missing: {:?}",
        code.spans
    );
    assert!(!all_text(&lines).contains("```"), "{}", all_text(&lines));
}

#[test]
fn markdown_parity_mermaid_renders_diagram_art() {
    let lines = assistant_lines("```mermaid\nflowchart TD\nA --> B\n```", 80);
    let text: Vec<String> = lines.iter().map(line_text).collect();
    assert!(
        text.iter().any(|l| l.contains('┌') && l.contains('┐')),
        "mermaid diagram boxes missing: {text:#?}"
    );
    assert!(
        text.iter().any(|l| l.contains("│ A │")),
        "mermaid node A missing: {text:#?}"
    );
}

#[test]
fn markdown_parity_latex_math_transforms() {
    use ratatui::style::Modifier;
    let lines = assistant_lines("$x^2$", 80);
    assert_eq!(line_text(&lines[0]), "ai ▸ x²");
    let math = lines[0]
        .spans
        .iter()
        .find(|s| s.content == "x²")
        .expect("math span missing");
    assert!(
        math.style.add_modifier.contains(Modifier::ITALIC),
        "{:?}",
        lines[0].spans
    );
}

#[test]
fn markdown_long_prose_wraps_to_feed_width() {
    use unicode_width::UnicodeWidthStr;
    let input = "word ".repeat(30);
    let lines = assistant_lines(&input, 20);
    assert!(lines.len() > 1, "{lines:#?}");
    for line in &lines {
        let width = line_text(line).width();
        assert!(width <= 20, "line too wide ({width}): {line:?}");
    }
    // All words survive the re-wrap (nothing dropped).
    assert_eq!(
        all_text(&lines).matches("word").count(),
        30,
        "{}",
        all_text(&lines)
    );
}

#[test]
fn markdown_inline_styles_survive_wrapping() {
    use ratatui::style::Modifier;
    // A bold/italic run inside prose that must wrap: span styles are
    // re-applied through the wrap byte ranges instead of degrading to the
    // line's base style.
    let input = format!(
        "{} **bold** *em* {}",
        "word ".repeat(10),
        "word ".repeat(10)
    );
    let lines = assistant_lines(&input, 20);
    assert!(lines.len() > 1, "{lines:#?}");
    let bold: String = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(bold, "bold", "{lines:#?}");
    let italic: String = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::ITALIC))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(italic, "em", "{lines:#?}");
}

#[test]
fn markdown_link_underline_survives_wrapping() {
    use ratatui::style::Modifier;
    use unicode_width::UnicodeWidthStr;
    // A link inside prose that must wrap: the underline comes from the
    // renderer's hyperlinks, projected through the wrap byte ranges.
    let input = format!(
        "{} [link](https://example.com) {}",
        "before ".repeat(4),
        "after ".repeat(4)
    );
    let lines = assistant_lines(&input, 20);
    for line in &lines {
        let width = line_text(line).width();
        assert!(width <= 20, "line too wide ({width}): {line:?}");
    }
    let underlined: String = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(underlined.contains("link"), "{underlined}");
    assert!(underlined.contains("https://"), "{underlined}");
    assert!(underlined.contains("example.com"), "{underlined}");
}

#[test]
fn markdown_prefix_only_on_first_rendered_line() {
    let lines = assistant_lines("hello\n\nworld", 80);
    let text: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(text[0], "ai ▸ hello", "{text:#?}");
    assert!(
        text.iter().skip(1).all(|l| !l.contains("ai ▸")),
        "{text:#?}"
    );
    assert!(text.iter().any(|l| l == "world"), "{text:#?}");
}

#[test]
fn feed_urls_get_underline_style() {
    use ratatui::style::Modifier;
    // A user block with an http URL: the URL span must carry UNDERLINED.
    let mut feed = theway_transport::feed::Feed::new();
    feed.push_user("see https://example.com/path now");
    let lines = crate::feed_render::lines(
        &feed,
        100,
        &crate::feed_render::FeedRenderOptions::default(),
    );
    let underlined: String = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(
        underlined.contains("https://example.com/path"),
        "{underlined}"
    );
}

#[test]
fn assistant_url_uses_hyperlinks_not_regex_scan() {
    use ratatui::style::Modifier;
    // Assistant blocks skip the regex URL scan: the underline comes from the
    // renderer's hyperlink output, so the pretty-mode link text itself is
    // underlined alongside the URL.
    let mut feed = theway_transport::feed::Feed::new();
    feed.push_assistant("[docs](https://example.com/x)");
    let lines = crate::feed_render::lines(
        &feed,
        100,
        &crate::feed_render::FeedRenderOptions::default(),
    );
    let underlined: String = lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(underlined.contains("docs"), "{underlined}");
    assert!(underlined.contains("https://example.com/x"), "{underlined}");
}

#[test]
fn selection_orders_direction_and_paint_cols_clamp_to_row_width() {
    // Reversed direction normalizes (rows first, then columns).
    let sel = FeedSelection {
        anchor: (5, 3),
        head: (2, 9),
    };
    assert_eq!(sel.ordered(), ((2, 9), (5, 3)));

    // A backward drag within one row normalizes by column.
    let row_sel = FeedSelection {
        anchor: (2, 3),
        head: (2, 1),
    };
    assert_eq!(row_sel.paint_cols(2, &Line::from("hello")), (1, 3));

    // Columns clamp to the row's text width — wide CJK chars count 2 each,
    // so a stored column 7 clamps to the 4-column text width.
    let wide = FeedSelection {
        anchor: (0, 7),
        head: (0, 7),
    };
    assert_eq!(wide.paint_cols(0, &Line::from("中中")), (4, 4));

    // Boundary rows paint their column slice; interior rows paint full
    // width; rows outside the selection paint nothing.
    let lines = [
        Line::from("hello world"),
        Line::from("mid"),
        Line::from("tail"),
    ];
    let sel = FeedSelection {
        anchor: (0, 6),
        head: (2, 2),
    };
    assert_eq!(sel.paint_cols(0, &lines[0]), (6, 11));
    assert_eq!(sel.paint_cols(1, &lines[1]), (0, 3));
    assert_eq!(sel.paint_cols(2, &lines[2]), (0, 2));
    assert_eq!(sel.paint_cols(3, &lines[0]), (0, 0));
}

#[test]
fn selection_to_capped_maps_uncapped_rows_and_drops_trimmed() {
    let sel = FeedSelection {
        anchor: (10, 2),
        head: (14, 3),
    };
    let capped = sel.to_capped(8, 100).unwrap();
    assert_eq!(capped.anchor, (2, 2));
    assert_eq!(capped.head, (6, 3));

    // Entirely above the trimmed head → gone; empty slice → gone.
    assert!(
        FeedSelection {
            anchor: (1, 0),
            head: (3, 0)
        }
        .to_capped(8, 100)
        .is_none()
    );
    assert!(sel.to_capped(0, 0).is_none());
}
