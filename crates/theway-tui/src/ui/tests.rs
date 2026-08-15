use super::theme::{BlockAlign, Theme};
use super::{App, AppConfig, collect_slash_commands, snake_loader};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::sync::Arc;
use std::time::Duration;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};
use theway_transport::client::GrpcClient;
use theway_transport::feed::WireFeedBlock;
use theway_transport::grpc::{GrpcState, serve_grpc};
use theway_transport::history::HistoryStore;
use theway_transport::testing::FakeSessionOps;
use theway_transport::wire::{WireCommand, WireContextUsage, WireSkillSnapshot, WireStatus};
use tokio::sync::{broadcast, mpsc};

fn fixture_status(feed_blocks: Vec<WireFeedBlock>) -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: vec![theway_transport::wire::ProviderGroup {
            provider: "anthropic".into(),
            has_credential: true,
            models: vec![theway_transport::wire::ModelEntry {
                id: "claude-x".into(),
                name: "Claude X".into(),
            }],
        }],
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        feed_blocks,
        feed_lines: Vec::new(),
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
    }
}

/// In-process gRPC fixture + App: the client drives a real server (the
/// same GrpcState shape the transport tests use), so submit/cancel/approve
/// round-trip through actual tonic frames.
async fn test_app() -> (App, mpsc::UnboundedReceiver<WireCommand>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
    let latest = Arc::new(parking_lot::Mutex::new(fixture_status(Vec::new())));
    let (event_tx, _) = broadcast::channel::<AgentJobEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<DagEvent>(16);
    let registry = AgentJobRegistry::new();
    let agent_fwd = {
        let mut rx = registry.subscribe();
        let fwd_tx = event_tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let _ = fwd_tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("AgentJobEvent broadcast lagged by {n}, skipping");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .abort_handle()
    };
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    let state = GrpcState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest,
        events: event_tx,
        dag_events: dag_event_tx,
        registry,
        dag_engine: Arc::new(DagEngine::new()),
        session_ops,
        session_id: Arc::new(std::sync::RwLock::new("sess-1".into())),
        agent_fwd,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = serve_grpc(listener, state);
    let _server = server;
    let client = GrpcClient::connect(&addr).await.unwrap();
    let initial = fixture_status(vec![WireFeedBlock::Plain {
        text: "banner".into(),
        level: theway_transport::feed::Level::Header,
        timestamp: None,
    }]);
    let app = App::new(AppConfig {
        client,
        initial,
        cwd: std::path::PathBuf::from("/tmp/theway"),
        history: HistoryStore::load_from(std::path::Path::new("/nonexistent-theway-history")),
        registry: crate::local_commands::local_registry(),
        pending_images: vec![],
    });
    let mut app = app;
    // App::new loads the real `~/.theway/theme.toml`; force the default so
    // tests never depend on the machine's theme file (theme-specific tests
    // set `app.theme` explicitly).
    app.theme = super::theme::Theme::default();
    (app, command_rx)
}

fn buffer_text(buf: &Buffer) -> String {
    let area = *buf.area();
    let mut rows = Vec::new();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        rows.push(row.trim_end().to_string());
    }
    rows.join("\n")
}

fn mouse_event(column: u16, row: u16, kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
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
        text.contains("ai ▸ hi there, the box is pinned"),
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
        status_row >= lines.len() - 5,
        "status rule should be pinned near the bottom (row {status_row} of {}):\n{text}",
        lines.len()
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
async fn busy_status_shows_snake_loader_with_elapsed() {
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
    assert!(
        text.contains("working 0."),
        "elapsed timer should follow the label (sub-second in test):\n{text}"
    );
    assert!(
        text.contains("2 queued"),
        "queue depth missing from the busy band:\n{text}"
    );
    // Single-row layout (issue #42): the 9-cell snake track starts at
    // x+1, the working label at x+12, and the stats share the same row.
    let status_area = app.last_status_area.unwrap();
    for c in 0..9u16 {
        assert_eq!(
            buf[(status_area.x + 1 + c, status_area.y)].symbol(),
            "●",
            "track cell {c} must render the snake glyph:\n{text}"
        );
    }
    assert_eq!(
        buf[(status_area.x + 12, status_area.y)].symbol(),
        "w",
        "working label must start at x+12:\n{text}"
    );
    assert!(
        text.contains("char/s"),
        "throughput stats must share the busy row:\n{text}"
    );
    let lines: Vec<&str> = text.lines().collect();
    let label_row = lines.iter().position(|l| l.contains("working")).unwrap();
    let border_row = lines
        .iter()
        .rposition(|l| l.contains('╭'))
        .expect("composer top border missing");
    assert_eq!(
        border_row,
        label_row + 1,
        "composer should sit directly below the single-row busy band:\n{text}"
    );
    // The busy window timer arms on the false→true edge and clears on idle.
    assert!(app.busy_started.is_some());
    let mut idle = fixture_status(Vec::new());
    idle.busy = false;
    app.apply_snapshot(idle);
    assert!(app.busy_started.is_none());
}

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

/// Head trajectory: the triangular wave walks 0→8→0 and bounces at both
/// ends of the fixed 9-cell track.
#[test]
fn snake_head_bounces_along_the_nine_cell_track() {
    let expected = [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 7, 6, 5, 4, 3, 2, 1];
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
    // Moving right: head at 8, the tail trails to its left.
    assert_eq!(snake_loader::segment_pos(8, 0), Some(8));
    assert_eq!(snake_loader::segment_pos(8, 1), Some(7));
    assert_eq!(snake_loader::segment_pos(8, 2), Some(6));
    // Step 9 reverses: head at 7, the tail now trails on the right.
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
/// 8; segments whose history predates the wave start stay dim.
#[test]
fn snake_trail_grows_from_two_to_eight_segments() {
    assert_eq!(snake_loader::trail_len(0.0), 2.0);
    assert_eq!(snake_loader::trail_len(1e9), 8.0);
    // Mid-wave step 16 (head at 0 moving right): the history is a
    // straight run, so the lit count equals the trail length.
    let idle = snake_loader::snake_frame(16, 0.0);
    assert_eq!(
        idle.cells.iter().filter(|c| c.lit > 0.0).count(),
        2,
        "idle trail must light 2 cells"
    );
    let fast = snake_loader::snake_frame(16, 1e9);
    assert_eq!(
        fast.cells.iter().filter(|c| c.lit > 0.0).count(),
        8,
        "speed-cap trail must light 8 cells"
    );
    // History predating the wave start renders dim: only the head lit.
    let early = snake_loader::snake_frame(0, 1e9);
    assert_eq!(early.cells.iter().filter(|c| c.lit > 0.0).count(), 1);
    assert_eq!(early.cells[0].lit, 1.0);
}

/// Track stability: all nine cells render every frame — lit cells carry
/// the rainbow body, unlit ones stay as dim dots on a dim background so
/// the single-row band never changes shape.
#[test]
fn snake_track_always_shows_all_nine_cells_with_dim_background() {
    for step in [0u64, 4, 8, 9, 15, 23, 100] {
        for cps in [0.0, 500.0, 1e9] {
            let frame = snake_loader::snake_frame(step, cps);
            assert_eq!(frame.cells.len(), 9, "step {step}");
            for (i, cell) in frame.cells.iter().enumerate() {
                assert_eq!(cell.glyph, '●', "step {step} cell {i}");
                if cell.lit > 0.0 {
                    assert_eq!(cell.bg, Color::Reset, "step {step} cell {i}");
                } else {
                    assert_eq!(cell.lit, 0.0, "step {step} cell {i}");
                    assert_ne!(
                        cell.bg,
                        Color::Reset,
                        "step {step} cell {i}: unlit track dots need a dim background"
                    );
                }
            }
        }
    }
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
        y >= status_y.saturating_sub(height) + 1 && y <= status_y.saturating_sub(2),
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
    assert!(!app.completions.contains(&"/skill::secrets-check".to_string()));

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

#[tokio::test]
async fn drag_on_status_rule_resizes_composer_and_send_resets() {
    let (mut app, _rx) = test_app().await;
    let backend = TestBackend::new(60, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let rule_row = app.last_status_area.unwrap().y;
    assert_eq!(app.composer_rows(60), 1);
    app.handle_mouse_down(mouse_event(
        5,
        rule_row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    assert!(app.resize_drag.is_some());
    app.handle_mouse_drag(5, rule_row.saturating_sub(4));
    assert_eq!(app.manual_composer_rows, Some(5));
    assert_eq!(app.composer_rows(60), 5);
    app.handle_mouse_up();
    assert!(app.resize_drag.is_none());
    // Sending resets the dragged height (issue #37).
    app.set_input("/quit");
    app.submit(&mut terminal_placeholder()).await.unwrap();
    assert!(app.manual_composer_rows.is_none());
    assert_eq!(app.composer_rows(60), 1);
    assert!(app.quit);
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

/// Drag override priority (issue #40): the mouse-dragged height outranks
/// both the computed wrap height and the content cap.
#[tokio::test]
async fn composer_rows_drag_override_wins_over_wrapped_height() {
    let (mut app, _rx) = test_app().await;
    app.set_input(&"x".repeat(200));
    let backend = TestBackend::new(60, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let rule_row = app.last_status_area.unwrap().y;
    assert_eq!(app.composer_rows(60), 4, "200 chars wrap into 4 rows");

    app.handle_mouse_down(mouse_event(
        5,
        rule_row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    app.handle_mouse_drag(5, rule_row.saturating_sub(3));
    assert_eq!(app.manual_composer_rows, Some(7));
    assert_eq!(
        app.composer_rows(60),
        7,
        "drag override must win over the computed 4 rows and the 6-row cap"
    );
    app.handle_mouse_up();
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
    app.handle_mouse_down(mouse_event(
        2,
        anchor_row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    assert!(app.feed_selection.is_some(), "down must start a selection");
    app.handle_mouse_drag(2, anchor_row + 3);
    let sel = app.feed_selection.unwrap();
    assert_eq!(
        sel.range(app.selection_view.total).count(),
        4,
        "drag must extend the selection over the rows crossed"
    );
    app.handle_mouse_up();
    assert!(
        app.feed_selection.is_some(),
        "selection persists after the button is released"
    );
    assert!(!app.mouse_selecting);
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
    ));
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
    ));
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

#[tokio::test]
async fn snapshot_with_unchanged_feed_keeps_local_annotations() {
    let (mut app, _rx) = test_app().await;
    let status = fixture_status(app.latest.feed_blocks.clone());
    app.system_line("local note");
    app.apply_snapshot(status);
    // feed_blocks unchanged → feed not rebuilt → the local note survives.
    let text = feed_text(&app);
    assert!(text.contains("local note"), "{text}");
}

#[tokio::test]
async fn snapshot_tail_append_pushes_only_new_blocks() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Plain {
        text: "banner".into(),
        level: theway_transport::feed::Level::System,
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    // Local annotations survive a pure tail append (no full rebuild).
    app.system_line("local note");
    let mut second = fixture_status(app.latest.feed_blocks.clone());
    second.feed_blocks.push(WireFeedBlock::Assistant {
        text: "appended answer".into(),
        timestamp: None,
    });
    app.apply_snapshot(second);
    let text = feed_text(&app);
    assert!(text.contains("banner"), "{text}");
    assert!(text.contains("appended answer"), "{text}");
    assert!(text.contains("local note"), "{text}");
}

#[tokio::test]
async fn snapshot_truncation_rebuilds_feed() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![
        WireFeedBlock::Plain {
            text: "one".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "two".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
    ]);
    app.apply_snapshot(first);
    // A shorter snapshot means the daemon truncated/reset the transcript —
    // prefix diff fails, the feed rebuilds from the new block list.
    let second = fixture_status(vec![WireFeedBlock::Plain {
        text: "fresh".into(),
        level: theway_transport::feed::Level::System,
        timestamp: None,
    }]);
    app.apply_snapshot(second);
    let text = feed_text(&app);
    assert!(text.contains("fresh"), "{text}");
    assert!(!text.contains("one"), "{text}");
    assert!(!text.contains("two"), "{text}");
}

#[tokio::test]
async fn submit_sends_message_to_daemon() {
    let (mut app, mut rx) = test_app().await;
    app.set_input("hello daemon");
    app.submit(&mut terminal_placeholder()).await.unwrap();
    match rx.recv().await.unwrap() {
        WireCommand::Submit {
            text,
            images,
            interrupt,
        } => {
            assert_eq!(text, "hello daemon");
            assert!(images.is_empty());
            assert!(!interrupt);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn slash_quit_sets_quit_and_clear_empties_feed() {
    let (mut app, _rx) = test_app().await;
    app.dispatch_slash("/quit", &mut terminal_placeholder())
        .await;
    assert!(app.quit);

    app.feed.push_user("stale");
    app.dispatch_slash("/clear", &mut terminal_placeholder())
        .await;
    assert!(feed_text(&app).is_empty());
}

#[tokio::test]
async fn nonlocal_slash_forwards_to_daemon() {
    let (mut app, mut rx) = test_app().await;
    app.dispatch_slash("/model anthropic:claude-x", &mut terminal_placeholder())
        .await;
    match rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => {
            assert_eq!(text, "/model anthropic:claude-x")
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn ctrl_c_while_busy_sends_cancel() {
    let (mut app, mut rx) = test_app().await;
    app.busy = true;
    app.request_abort();
    // cancel is fired on a spawned task; drain the command channel.
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no cancel command")
        .unwrap();
    assert!(matches!(cmd, WireCommand::Abort));
}

#[tokio::test]
async fn control_plane_prompt_key_approves_via_rpc() {
    let (mut app, mut rx) = test_app().await;
    app.control_plane_prompt = Some(theway_transport::wire::WireControlPlanePromptSnapshot {
        tool_name: "write".into(),
        label: "write file".into(),
        reason: "needs approval".into(),
        args_hash: "abc".into(),
        payload: "{}".into(),
    });
    assert!(
        app.handle_control_plane_prompt_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
    );
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no approve command")
        .unwrap();
    match cmd {
        WireCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn model_picker_alt_m_selects_and_sends_set_model() {
    let (mut app, mut rx) = test_app().await;
    app.open_model_picker();
    assert!(app.model_picker.is_some());

    // Enter descends into anthropic; Enter again selects the first model.
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no set_model command")
        .unwrap();
    match cmd {
        WireCommand::SetModel { spec } => {
            assert_eq!(spec, "anthropic:claude-x")
        }
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(app.model_picker.is_none());
}

#[tokio::test]
async fn session_switch_sends_switch_session_rpc() {
    let (mut app, mut rx) = test_app().await;
    app.dispatch_slash("/session switch sess-1", &mut terminal_placeholder())
        .await;
    let cmd = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("no switch_session command")
        .unwrap();
    match cmd {
        WireCommand::SwitchSession { id } => assert_eq!(id, "sess-1"),
        other => panic!("unexpected command: {other:?}"),
    }
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

/// Tests drive `App` methods that only borrow the terminal (never draw);
/// a stdout-backed terminal is fine without entering raw mode.
fn terminal_placeholder() -> Terminal<CrosstermBackend<std::io::Stdout>> {
    Terminal::new(CrosstermBackend::new(std::io::stdout())).unwrap()
}

fn assistant_lines(text: &str, width: usize) -> Vec<ratatui::text::Line<'static>> {
    let mut out: Vec<ratatui::text::Line<'static>> = Vec::new();
    crate::feed_render::push_markdown(
        &mut out,
        text,
        "ai ▸ ",
        ratatui::style::Style::default(),
        width,
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
fn feed_selection_range_and_extend_clamp() {
    let mut sel = super::FeedSelection { anchor: 10, end: 5 };
    // Ordered inclusive range.
    assert_eq!(sel.range(100), 5..=10);
    // Clamped to the last valid line.
    assert_eq!(sel.range(8), 5..=7);
    // Extend up clamps at 0, down clamps at total-1.
    sel.extend(-100, 100);
    assert_eq!(sel.end, 0);
    sel.end = 95;
    sel.extend(100, 100);
    assert_eq!(sel.end, 99);
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
