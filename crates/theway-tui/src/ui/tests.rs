use super::{App, AppConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
use theway_transport::wire::{WireCommand, WireContextUsage, WireStatus};
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
async fn busy_status_shows_pixel_loader_with_elapsed() {
    let (mut app, _rx) = test_app().await;
    let mut status = fixture_status(Vec::new());
    status.busy = true;
    status.queued_count = 2;
    app.apply_snapshot(status);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("working"), "busy label missing:\n{text}");
    assert!(
        text.contains("working 0."),
        "elapsed timer should follow the label (sub-second in test):\n{text}"
    );
    assert!(
        text.contains("2 queued"),
        "queue depth missing from the busy band:\n{text}"
    );
    assert!(
        text.contains('■'),
        "pixel-grid glyphs missing from the busy band:\n{text}"
    );
    // The busy band is 3 rows: composer top border sits 2 rows below the
    // middle (label) row.
    let lines: Vec<&str> = text.lines().collect();
    let label_row = lines.iter().position(|l| l.contains("working")).unwrap();
    let border_row = lines
        .iter()
        .rposition(|l| l.contains('╭'))
        .expect("composer top border missing");
    assert_eq!(
        border_row,
        label_row + 2,
        "composer should sit below the 3-row loader band:\n{text}"
    );
    // The busy window timer arms on the false→true edge and clears on idle.
    assert!(app.busy_started.is_some());
    let mut idle = fixture_status(Vec::new());
    idle.busy = false;
    app.apply_snapshot(idle);
    assert!(app.busy_started.is_none());
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
    assert_eq!(app.composer_rows(), 1);
    app.handle_mouse_down(mouse_event(
        5,
        rule_row,
        MouseEventKind::Down(MouseButton::Left),
    ));
    assert!(app.resize_drag.is_some());
    app.handle_mouse_drag(5, rule_row.saturating_sub(4));
    assert_eq!(app.manual_composer_rows, Some(5));
    assert_eq!(app.composer_rows(), 5);
    app.handle_mouse_up();
    assert!(app.resize_drag.is_none());
    // Sending resets the dragged height (issue #37).
    app.set_input("/quit");
    app.submit(&mut terminal_placeholder()).await.unwrap();
    assert!(app.manual_composer_rows.is_none());
    assert_eq!(app.composer_rows(), 1);
    assert!(app.quit);
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
    assert!(super::feature_labels(&[], &[], false).is_empty());
}

#[test]
fn feature_labels_passes_runtime_features_through() {
    let runtime = vec!["suppress".to_string(), "cycle".to_string()];
    assert_eq!(super::feature_labels(&runtime, &[], false), runtime);
}

#[test]
fn feature_labels_derives_graph_engine_from_dag_run() {
    let labels = super::feature_labels(&[], &[dag_run("dag")], false);
    assert_eq!(labels, vec!["graph engine".to_string()]);
}

#[test]
fn feature_labels_goal_from_run_or_active_goal_once() {
    let from_run = super::feature_labels(&[], &[dag_run("goal")], false);
    assert_eq!(from_run, vec!["goal".to_string()]);
    let from_goal = super::feature_labels(&[], &[], true);
    assert_eq!(from_goal, vec!["goal".to_string()]);
    // Both sources active still emit a single label.
    let both = super::feature_labels(&[], &[dag_run("goal")], true);
    assert_eq!(both, vec!["goal".to_string()]);
}

#[test]
fn feature_labels_combined_order() {
    let runtime = vec!["inject-and-run".to_string()];
    let dags = vec![dag_run("dag"), dag_run("goal")];
    let labels = super::feature_labels(&runtime, &dags, true);
    assert_eq!(
        labels,
        vec![
            "inject-and-run".to_string(),
            "graph engine".to_string(),
            "goal".to_string(),
        ]
    );
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
    status.sidebar.runtime = vec!["inject-and-run".to_string()];
    status.dags = vec![dag_run("dag")];
    status.goal = Some(theway_transport::wire::WireGoalSnapshot {
        condition: "done".into(),
        status: "running".into(),
        iterations: 1,
        last_reason: None,
    });
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
        divider.contains("inject-and-run · graph engine · goal"),
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
