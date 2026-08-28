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
        thinking_level: "off".into(),
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
        session_usage: WireContextUsage::default(),
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
        model_config_path: std::path::PathBuf::from("/nonexistent-theway-config/config.toml"),
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

/// Drains the fixture's command channel on a background task and answers
/// oneshot RPCs (`SetModel` / `SetThinking`) so `client.set_model(...)` /
/// `client.set_thinking(...)` complete. The in-process gRPC server waits for
/// the oneshot response, so a test that both awaits the RPC and reads the
/// channel itself would deadlock — the drainer breaks that cycle.
/// Returns the task handle plus a label list of every command seen
/// (`SetModel(anthropic:claude-x)`, `SetThinking(high)`, `Submit(/model list)`).
fn drain_commands(
    mut rx: mpsc::UnboundedReceiver<WireCommand>,
) -> (
    tokio::task::JoinHandle<()>,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let collector = seen.clone();
    let handle = tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            let label = match command {
                WireCommand::SetModel { spec, response, .. } => {
                    let _ = response.send(true);
                    format!("SetModel({spec})")
                }
                WireCommand::SetThinking {
                    level, response, ..
                } => {
                    let _ = response.send(true);
                    format!("SetThinking({level})")
                }
                WireCommand::Submit { text, .. } => format!("Submit({text})"),
                other => format!("{other:?}"),
            };
            collector.lock().unwrap().push(label);
        }
    });
    (handle, seen)
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
fn terminal_lifecycle_enables_mouse_and_paste_capture() {
    let mut enter = Vec::new();
    super::render_utils::write_enter_tui_commands(&mut enter).unwrap();
    let mut leave = Vec::new();
    super::render_utils::write_leave_tui_commands(&mut leave).unwrap();

    assert!(enter.windows(8).any(|bytes| bytes == b"\x1b[?2004h"));
    assert!(leave.windows(8).any(|bytes| bytes == b"\x1b[?2004l"));
    // Mouse capture (SGR mode) is enabled on enter so the wheel can scroll
    // (works under tmux, which forwards app mouse tracking), and disabled
    // on leave so the terminal keeps its own mouse mode untouched.
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
        status_row >= lines.len() - 5,
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
    "/tests/ui/unit/stats.rs"
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
    "/tests/ui/unit/model.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/runtime.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/rendering.rs"
));
