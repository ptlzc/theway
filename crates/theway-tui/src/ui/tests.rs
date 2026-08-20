use super::theme::{BlockAlign, Theme};
use super::{App, AppConfig, collect_slash_commands, snake_loader};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::sync::Arc;
use std::time::Duration;
use theway_transport::client::GrpcClient;
use theway_transport::feed::WireFeedBlock;
use theway_transport::grpc::{GrpcState, serve_grpc};
use theway_transport::history::HistoryStore;
use theway_transport::testing::FakeSessionOps;
use theway_transport::wire::{
    WireCommand, WireContextUsage, WireDaemonConfig, WireFeedBlockPatch, WirePathContext,
    WireSkillSnapshot, WireStatus,
};
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
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: Vec::new(),
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: theway_transport::wire::WireExtensionSnapshot::default(),
    }
}

/// In-process gRPC fixture + App: the client drives a real server (the
/// same GrpcState shape the transport tests use), so submit/cancel/approve
/// round-trip through actual tonic frames.
async fn test_app() -> (App, mpsc::UnboundedReceiver<WireCommand>) {
    let (app, rx, _ops) = test_app_with_sessions(&["sess-1"]).await;
    (app, rx)
}

/// [`test_app`] with an explicit seed session list (empty = a daemon with
/// no sessions) plus the `FakeSessionOps` handle for tests that inspect or
/// mutate the session table (issue #56).
async fn test_app_with_sessions(
    seeds: &[&str],
) -> (
    App,
    mpsc::UnboundedReceiver<WireCommand>,
    Arc<FakeSessionOps>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<theway_transport::wire::WireStatusUpdate>(16);
    let latest = Arc::new(parking_lot::Mutex::new(fixture_status(Vec::new())));
    let (event_tx, _) = broadcast::channel::<theway_transport::wire::WireAgentEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<theway_transport::wire::WireDagEvent>(16);
    let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
    let session_ops = Arc::new(FakeSessionOps::new());
    for id in seeds {
        session_ops.add_session(id);
    }
    let current: String = seeds.first().copied().unwrap_or("").to_string();
    let state = GrpcState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest,
        events: event_tx,
        dag_events: dag_event_tx,
        job_ops: Arc::new(theway_transport::UnavailableJobOps),
        graph_ops: Arc::new(theway_transport::UnavailableGraphOps),
        session_ops: session_ops.clone(),
        session_id: Arc::new(std::sync::RwLock::new(current)),
        agent_fwd,
        path_context: Arc::new(std::sync::RwLock::new(WirePathContext::default())),
        daemon_config: Arc::new(std::sync::RwLock::new(WireDaemonConfig::default())),
        tool_ops: Arc::new(theway_transport::UnavailableToolOps),
        storage_ops: Arc::new(theway_transport::UnavailableStorageOps),
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
        connector: None,
        initial,
        cwd: std::path::PathBuf::from("/tmp/theway"),
        history: HistoryStore::load_from(std::path::Path::new("/nonexistent-theway-history")),
        registry: crate::local_commands::local_registry(),
        pending_images: vec![],
        color_level: theway_markdown::ColorLevel::TrueColor,
    });
    let mut app = app;
    // App::new loads the real `~/.theway/theme.toml`; force the default so
    // tests never depend on the machine's theme file (theme-specific tests
    // set `app.theme` explicitly).
    app.theme = super::theme::Theme::default();
    (app, command_rx, session_ops)
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

#[test]
fn terminal_lifecycle_does_not_enable_mouse_tracking() {
    let mut enter = Vec::new();
    super::render_utils::write_enter_tui_commands(&mut enter).unwrap();
    let mut leave = Vec::new();
    super::render_utils::write_leave_tui_commands(&mut leave).unwrap();

    assert!(enter.windows(8).any(|bytes| bytes == b"\x1b[?2004h"));
    assert!(leave.windows(8).any(|bytes| bytes == b"\x1b[?2004l"));
    for mode in [b"\x1b[?1000".as_slice(), b"\x1b[?1002", b"\x1b[?1006"] {
        assert!(
            !enter.windows(mode.len()).any(|bytes| bytes == mode),
            "TUI enter must leave mouse mode {mode:?} to the terminal"
        );
        assert!(
            !leave.windows(mode.len()).any(|bytes| bytes == mode),
            "TUI leave must not mutate terminal mouse mode {mode:?}"
        );
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
async fn busy_status_shows_braille_spinner_with_elapsed() {
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
        text.contains("char/s"),
        "throughput stats must share the busy row:\n{text}"
    );
    let first_color = buf[(status_area.x + 1, status_area.y)].fg;
    let lines: Vec<&str> = text.lines().collect();
    let label_row = lines.iter().position(|l| l.contains("working")).unwrap();
    let border_row = lines
        .iter()
        .rposition(|l| l.contains('╭'))
        .expect("composer top border missing");
    assert_eq!(
        border_row,
        label_row + 1,
        "composer should sit directly below the compact busy band:\n{text}"
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
    // The busy window timer arms on the false→true edge and clears on idle.
    assert!(app.busy_started.is_some());
    let mut idle = fixture_status(Vec::new());
    idle.busy = false;
    app.apply_snapshot(idle);
    assert!(app.busy_started.is_none());
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/status.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/layout.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/sessions.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/runtime.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/rendering.rs"
));
