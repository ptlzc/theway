use super::{App, AppConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
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
use theway_transport::wire::{WireCommand, WireStatus};
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
        dags: Vec::new(),
        subagents: Vec::new(),
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
        text.contains("you ▸ hello world"),
        "feed user line missing:\n{text}"
    );
    assert!(
        text.contains("ai ▸ hi there, the box is pinned"),
        "assistant line missing:\n{text}"
    );
    assert!(text.contains("theway ·"), "status rule missing:\n{text}");
    assert!(
        text.contains("ready"),
        "status should read ready when idle:\n{text}"
    );
    let lines: Vec<&str> = text.lines().collect();
    let status_row = lines.iter().position(|l| l.contains("theway ·")).unwrap();
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
async fn status_line_shows_busy_spinner_from_snapshot() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.busy = true;
    app.apply_snapshot(status);
    let backend = TestBackend::new(50, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("working (Ctrl-C"),
        "busy status missing:\n{text}"
    );
}

#[tokio::test]
async fn snapshot_rebuilds_feed_and_resyncs_busy_panel() {
    let (mut app, _rx) = test_app().await;
    assert!(
        crate::feed_render::lines(&app.feed, 100)
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
    app.apply_snapshot(status);

    assert!(app.busy);
    assert_eq!(app.latest.queued_count, 2);
    let text = feed_text(&app);
    assert!(text.contains("you ▸ snap question"), "{text}");
    assert!(text.contains("ai ▸ snap answer"), "{text}");
    // The old banner block is gone (whole-replacement semantics).
    assert!(!text.contains("banner"), "{text}");
    assert!(app.follow);
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
    crate::feed_render::lines(&app.feed, 100)
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

#[test]
fn markdown_single_tilde_pair_stays_literal() {
    use crate::feed_render::push_markdown_paragraphs;
    use ratatui::style::Style;
    let mut lines: Vec<ratatui::text::Line<'static>> = Vec::new();
    push_markdown_paragraphs(
        &mut lines,
        "~**10%** is not struck",
        Style::default(),
        None,
        80,
    );
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    // Single-tilde pairs are demoted to literal `~` by the shared parser
    // options — the renderer must not strike them.
    assert!(text.contains("~**10%**"), "{text}");
}

#[test]
fn markdown_fenced_code_renders_verbatim_no_wrap() {
    use crate::feed_render::push_markdown_paragraphs;
    use ratatui::style::Style;
    // A code line longer than the width must stay on one unwrapped line.
    let long_code = "let x = 1; // ".to_string() + &"a".repeat(120);
    let input = format!("before\n```rust\n{long_code}\n```\nafter");
    let mut lines: Vec<ratatui::text::Line<'static>> = Vec::new();
    push_markdown_paragraphs(&mut lines, &input, Style::default(), None, 40);
    let rendered: Vec<String> = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .map(String::from)
        .collect();
    assert!(
        rendered
            .iter()
            .any(|l| l.contains("let x = 1;") && l.len() >= long_code.len()),
        "{rendered:#?}"
    );
    assert!(rendered.iter().any(|l| l == "```rust"), "{rendered:#?}");
}

#[test]
fn feed_urls_get_underline_style() {
    use ratatui::style::Modifier;
    // A user block with an http URL: the URL span must carry UNDERLINED.
    let mut feed = theway_transport::feed::Feed::new();
    feed.push_user("see https://example.com/path now");
    let lines = crate::feed_render::lines(&feed, 100);
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
