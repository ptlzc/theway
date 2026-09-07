use super::*;

#[test]
fn terminal_lifecycle_enables_mouse_and_paste_capture() {
    let mut enter = Vec::new();
    super::render_utils::write_enter_tui_commands(&mut enter).unwrap();
    let mut leave = Vec::new();
    super::render_utils::write_leave_tui_commands(&mut leave).unwrap();

    assert!(enter.windows(8).any(|bytes| bytes == b"\x1b[?2004h"));
    assert!(leave.windows(8).any(|bytes| bytes == b"\x1b[?2004l"));
    // Mouse capture (SGR mode) is enabled on enter so the wheel can scroll
    // the TUI feed, and disabled on leave so the terminal keeps its own
    // mouse mode untouched.
    assert!(enter.windows(8).any(|bytes| bytes == b"\x1b[?1006h"));
    assert!(leave.windows(8).any(|bytes| bytes == b"\x1b[?1006l"));
}

#[tokio::test]
async fn renders_feed_above_pinned_input_box() {
    let (mut app, _rx) = test_app().await;
    app.feed.push_user("hello world");
    app.feed.push_assistant("hi there, the box is pinned");

    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());

    assert!(
        text.contains("❯ hello world"),
        "feed user line missing:\n{text}"
    );
    assert!(
        text.contains("hi there, the box is pinned"),
        "assistant line missing:\n{text}"
    );
    assert!(
        text.contains("ready"),
        "status should read ready when idle:\n{text}"
    );
    // Issue #37: the status rule carries no brand or model label anymore.
    assert!(
        !text.contains("theway ·"),
        "brand label must be gone from the status rule:\n{text}"
    );
    let lines: Vec<&str> = text.lines().collect();
    let status_row = lines.iter().position(|l| l.contains("ready")).unwrap();
    assert!(
        status_row >= lines.len() - 6,
        "status rule should be pinned near the bottom (row {status_row} of {}):\n{text}",
        lines.len()
    );
}

#[tokio::test]
async fn screen_margin_insets_everything_from_the_terminal_edges() {
    let (mut app, _rx) = test_app().await;
    app.feed.push_user("hello world");
    // Left-biased margin: the UI hugging the terminal's left edge is the
    // complaint this feature addresses.
    app.theme.screen.margin_top = 1;
    app.theme.screen.margin_left = 3;
    app.theme.screen.margin_right = 2;

    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();

    // Top margin: the first terminal row stays blank.
    let row0: String = (0..50).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    assert_eq!(row0.trim(), "", "top margin row must be blank: {row0:?}");

    // Left margin: the feed's first line starts at column 3, not column 0.
    let row1: String = (0..50).map(|x| buf[(x, 1)].symbol().to_string()).collect();
    assert!(
        row1.starts_with("   "),
        "feed must start at the left margin, got: {row1:?}"
    );

    // The user prompt is indented by the same left margin.
    let prompt_row = (0..12)
        .map(|y| {
            (0..50)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .position(|line| line.contains("❯ hello world"))
        .expect("user line must render");
    let prompt_line: String = (0..50)
        .map(|x| buf[(x, prompt_row as u16)].symbol().to_string())
        .collect();
    assert!(
        prompt_line.starts_with("   ❯"),
        "user prompt must sit at the left margin, got: {prompt_line:?}"
    );
}

#[tokio::test]
async fn default_left_margin_shifts_ui_two_columns() {
    let (mut app, _rx) = test_app().await;
    app.feed.push_user("hello world");
    // No theme overrides: the built-in default left margin applies.
    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();

    // The first feed row starts at column 2, not column 0.
    let user_row = (0..12)
        .map(|y| {
            (0..50)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .position(|line| line.contains("❯ hello world"))
        .expect("user line must render");
    let row: String = (0..50)
        .map(|x| buf[(x, user_row as u16)].symbol().to_string())
        .collect();
    assert!(
        row.starts_with("  "),
        "feed must start at the default left margin (2), got: {row:?}"
    );
    assert!(!row.starts_with("   "), "margin must be 2, got: {row:?}");

    // The composer top divider also starts at column 2.
    let divider: String = (0..12)
        .map(|y| {
            (0..50)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .find(|line| line.contains('╭'))
        .expect("top divider must render");
    assert!(
        divider.trim_start().starts_with('╭'),
        "divider must sit at the left margin, got: {divider:?}"
    );
    assert_eq!(
        divider.find('╭'),
        Some(2),
        "divider must start at column 2, got: {divider:?}"
    );
}

#[tokio::test]
async fn status_line_shows_daemon_offline_when_disconnected() {
    let (mut app, _rx) = test_app().await;
    app.connected = false;
    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("daemon offline"),
        "offline banner missing:\n{text}"
    );
}

#[tokio::test]
async fn chrome_info_line_shows_model_with_provider() {
    let (mut app, _rx) = test_app().await;
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    // Issue #37: the composer info line carries the full provider:model-id
    // label (fixture model is `provider:model`).
    assert!(
        text.contains("provider:model"),
        "info line must show the model with provider:\n{text}"
    );
}

#[tokio::test]
async fn busy_status_shows_braille_spinner() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.busy = true;
    status.queued_count = 2;
    app.apply_snapshot(status);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let text = buffer_text(buf);
    assert!(text.contains("working"), "busy label missing:\n{text}");
    // Pi's Braille spinner stays in one terminal cell while the busy band
    // remains one row high.
    let status_area = app.last_status_area.unwrap();
    assert_eq!(status_area.height, 1);
    assert_eq!(buf[(status_area.x + 1, status_area.y)].symbol(), "⠋");
    assert_eq!(buf[(status_area.x + 2, status_area.y)].symbol(), " ");
    assert_eq!(
        buf[(status_area.x + 4, status_area.y)].symbol(),
        "w",
        "working label must start beside the Braille spinner:\n{text}"
    );
    let working_cells = (0.."working".len() as u16)
        .map(|offset| {
            let cell = &buf[(status_area.x + 4 + offset, status_area.y)];
            (cell.symbol().to_owned(), cell.fg, cell.bg, cell.modifier)
        })
        .collect::<Vec<_>>();
    assert!(
        text.contains("t/s"),
        "throughput stats must share the busy row:\n{text}"
    );
    let first_color = buf[(status_area.x + 1, status_area.y)].fg;
    let lines: Vec<&str> = text.lines().collect();
    let label_row = lines.iter().position(|l| l.contains("working")).unwrap();
    let border_row = lines
        .iter()
        .rposition(|l| l.contains('╭'))
        .expect("composer top border missing");
    // The composer now starts directly below the status bar (no spacer).
    assert_eq!(
        border_row,
        label_row + 1,
        "composer should sit directly below the busy band:\n{text}"
    );
    // Advance one base-cadence step: the mask and hue change in place.
    app.spinner.tick(130);
    terminal.draw(|f| app.render(f)).unwrap();
    let moved = terminal.backend().buffer();
    assert_eq!(moved[(status_area.x + 1, status_area.y)].symbol(), "⠙");
    assert_ne!(moved[(status_area.x + 1, status_area.y)].fg, first_color);
    assert_eq!(moved[(status_area.x + 2, status_area.y)].symbol(), " ");
    let moved_working_cells = (0.."working".len() as u16)
        .map(|offset| {
            let cell = &moved[(status_area.x + 4 + offset, status_area.y)];
            (cell.symbol().to_owned(), cell.fg, cell.bg, cell.modifier)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        moved_working_cells, working_cells,
        "working label must not change style between spinner frames"
    );
}
