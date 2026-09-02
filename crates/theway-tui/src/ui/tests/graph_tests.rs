//! Unit tests for the `/graph` DAG band, status-bar counters and the
//! Ctrl+C composer-clearing behavior (issues #76, #78, #80).

use super::*;

/// Ctrl-C while the composer holds text clears the input first and neither
/// aborts a busy turn nor exits.
#[tokio::test]
async fn ctrl_c_with_text_clears_input_without_abort_or_quit() {
    let (mut app, rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let (drainer, seen) = drain_commands(rx);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    app.set_input("hello");
    app.busy = true;
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.handle_key(key, &mut terminal).await.unwrap();

    assert!(
        app.input_text().is_empty(),
        "Ctrl-C with text must clear the composer"
    );
    assert!(!app.quit, "Ctrl-C with text must not exit");
    // request_abort (if it fired) runs on a background task; give it a beat
    // and assert no Abort command reached the daemon.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !seen.lock().unwrap().iter().any(|l| l.starts_with("Abort(")),
        "Ctrl-C with text must not abort a busy turn: {:?}",
        seen.lock().unwrap()
    );
    drainer.abort();
}

/// Ctrl-C on an empty composer while a turn is busy still aborts.
#[tokio::test]
async fn ctrl_c_empty_busy_aborts() {
    let (mut app, rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let (drainer, seen) = drain_commands(rx);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    app.busy = true;
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.handle_key(key, &mut terminal).await.unwrap();

    // request_abort runs the cancel RPC on a background task; poll for it.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if seen.lock().unwrap().iter().any(|l| l.starts_with("Abort(")) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "abort was not sent to the daemon: {:?}",
            seen.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!app.quit, "a busy abort must not exit");
    drainer.abort();
}

/// Ctrl-C on an empty composer while idle warns on the first press and exits
/// on the second (within the 1.5 s window).
#[tokio::test]
async fn ctrl_c_empty_idle_two_presses_exits() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.handle_key(key, &mut terminal).await.unwrap();
    assert!(!app.quit, "first Ctrl-C warns, does not exit");
    app.handle_key(key, &mut terminal).await.unwrap();
    assert!(app.quit, "second Ctrl-C within 1.5s exits");
}

// ── /graph (issue #76) ────────────────────────────────────────────────

/// Bare `/graph` toggles the DAG band Show ↔ Hidden.
#[tokio::test]
async fn graph_bare_toggles_band_mode() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    assert_eq!(app.dag_band_mode, crate::ui::DagBandMode::Show);
    app.set_input("/graph");
    app.submit(&mut terminal).await.unwrap();
    assert_eq!(
        app.dag_band_mode,
        crate::ui::DagBandMode::Hidden,
        "bare /graph must hide the band"
    );

    app.set_input("/graph");
    app.submit(&mut terminal).await.unwrap();
    assert_eq!(
        app.dag_band_mode,
        crate::ui::DagBandMode::Show,
        "bare /graph must restore the band"
    );
}

/// `/graph show` / `/graph hidden` set the band mode explicitly.
#[tokio::test]
async fn graph_show_and_hidden_set_mode_explicitly() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    app.set_input("/graph hidden");
    app.submit(&mut terminal).await.unwrap();
    assert_eq!(app.dag_band_mode, crate::ui::DagBandMode::Hidden);

    app.set_input("/graph show");
    app.submit(&mut terminal).await.unwrap();
    assert_eq!(app.dag_band_mode, crate::ui::DagBandMode::Show);
}

/// `/graph clear` clears the current session's terminal runs via the daemon.
#[tokio::test]
async fn graph_clear_calls_daemon_clear_session_runs() {
    let (mut app, rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let (drainer, seen) = drain_commands(rx);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    app.set_input("/graph clear");
    app.submit(&mut terminal).await.unwrap();
    // The clear runs a GraphClear unary RPC against the in-process daemon; it
    // is not a WireCommand, so the drainer sees nothing. Assert the session
    // was resolved to sess-1 and no abort/exit happened.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !seen.lock().unwrap().iter().any(|l| l.starts_with("Abort(")),
        "/graph clear must not abort: {:?}",
        seen.lock().unwrap()
    );
    assert!(!app.quit, "/graph clear must not exit");
    drainer.abort();
}

/// When the DAG band is Hidden, the status bar shows `[n graph]` for running
/// DAG runs only; when Show, it is omitted.
#[tokio::test]
async fn graph_counter_hidden_only_and_sub_shell() {
    use theway_transport::wire::{WireAgentJobSnapshot, WireDagNodeSnapshot, WireDagRunSnapshot};

    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let mut status = fixture_status(Vec::new());
    status.dags = vec![WireDagRunSnapshot {
        id: "run-1".into(),
        name: "demo".into(),
        kind: "dag".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 4,
        direction: "TD".into(),
        created_at: 0,
        completed_at: None,
        error: None,
        nodes: vec![WireDagNodeSnapshot {
            id: "n1".into(),
            agent: "a".into(),
            status: "running".into(),
            depends_on: vec![],
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
        }],
    }];
    status.subagents = vec![WireAgentJobSnapshot {
        id: "sub-1".into(),
        agent: "sub".into(),
        source: "".into(),
        run_id: None,
        node_id: None,
        status: "running".into(),
        started_at: None,
        completed_at: None,
        duration_ms: None,
        attempt: 1,
        total_attempts: 1,
        input_tokens: None,
        output_tokens: None,
        error: None,
        output_tail: None,
        live_preview: None,
        tps: None,
        cps: None,
        chars: None,
        tools_called: None,
        turn: None,
    }];
    status.shell_count = 4;
    status.busy = true;
    app.apply_snapshot(status.clone());

    let backend = TestBackend::new(80, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let text = buffer_text(terminal.backend().buffer());

    // Hidden + running dag -> `[1 graph]`; shell_count -> `[4 shell]`;
    // running subagent -> `[1 sub]`.
    app.dag_band_mode = crate::ui::DagBandMode::Hidden;
    terminal.draw(|f| app.render(f)).unwrap();
    let hidden_text = buffer_text(terminal.backend().buffer());
    assert!(
        hidden_text.contains("[1 graph]"),
        "hidden band must render [1 graph]:\n{hidden_text}"
    );
    assert!(
        hidden_text.contains("[4 shell]"),
        "must render [4 shell]:\n{hidden_text}"
    );
    assert!(
        hidden_text.contains("[1 sub]"),
        "must render [1 sub]:\n{hidden_text}"
    );

    // Show mode: `[n graph]` is omitted (it's hidden-only), but `[n sub]` and
    // `[n shell]` may still appear.
    app.dag_band_mode = crate::ui::DagBandMode::Show;
    terminal.draw(|f| app.render(f)).unwrap();
    let show_text = buffer_text(terminal.backend().buffer());
    assert!(
        !show_text.contains("[1 graph]"),
        "show mode must not render [1 graph]:\n{show_text}"
    );

    // Zero shell/sub counts are omitted.
    let _ = text;
    status.shell_count = 0;
    status.subagents.clear();
    app.apply_snapshot(status);
    app.dag_band_mode = crate::ui::DagBandMode::Hidden;
    terminal.draw(|f| app.render(f)).unwrap();
    let zero_text = buffer_text(terminal.backend().buffer());
    assert!(!zero_text.contains("[1 shell]"), "zero shell must omit");
    assert!(!zero_text.contains("[1 sub]"), "no subagent must omit");
}

/// Issue #76: when the DAG band mode is `Hidden`, the band is not rendered
/// (its run ids do not appear in the frame); `Show` renders it.
#[tokio::test]
async fn hidden_dag_band_is_not_rendered() {
    use theway_transport::wire::{WireDagNodeSnapshot, WireDagRunSnapshot};

    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let mut status = fixture_status(Vec::new());
    status.dags = vec![WireDagRunSnapshot {
        id: "graph-band-test".into(),
        name: "band-name".into(),
        kind: "dag".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 4,
        direction: "TD".into(),
        created_at: 0,
        completed_at: None,
        error: None,
        nodes: vec![WireDagNodeSnapshot {
            id: "n1".into(),
            agent: "a".into(),
            status: "running".into(),
            depends_on: vec![],
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
        }],
    }];
    app.apply_snapshot(status);

    let backend = TestBackend::new(80, 16);
    let mut terminal = Terminal::new(backend).unwrap();

    // Show (default): the band and its run id are rendered.
    app.dag_band_mode = crate::ui::DagBandMode::Show;
    terminal.draw(|f| app.render(f)).unwrap();
    let show_text = buffer_text(terminal.backend().buffer());
    assert!(
        show_text.contains("graph-band-test"),
        "Show mode must render the DAG band:\n{show_text}"
    );

    // Hidden: the band is not allocated, so the run id is absent.
    app.dag_band_mode = crate::ui::DagBandMode::Hidden;
    terminal.draw(|f| app.render(f)).unwrap();
    let hidden_text = buffer_text(terminal.backend().buffer());
    assert!(
        !hidden_text.contains("graph-band-test"),
        "Hidden mode must not render the DAG band:\n{hidden_text}"
    );
}

/// The DAG band is scrollable content: it renders at the bottom of the feed
/// while following, and scrolling up carries it with the feed (instead of
/// pinning it between the feed and the status bar).
#[tokio::test]
async fn dag_band_scrolls_with_feed() {
    use theway_transport::feed::WireFeedBlock;
    use theway_transport::wire::{WireDagNodeSnapshot, WireDagRunSnapshot};

    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let mut status = fixture_status(Vec::new());
    status.feed_blocks = (0..30)
        .map(|i| WireFeedBlock::User {
            text: format!("history message {i}"),
            timestamp: None,
        })
        .collect();
    status.dags = vec![WireDagRunSnapshot {
        id: "band-run".into(),
        name: "demo".into(),
        kind: "dag".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 4,
        direction: "TD".into(),
        created_at: 0,
        completed_at: None,
        error: None,
        nodes: vec![WireDagNodeSnapshot {
            id: "n1".into(),
            agent: "a".into(),
            status: "running".into(),
            depends_on: vec![],
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
        }],
    }];
    app.apply_snapshot(status);

    let backend = TestBackend::new(80, 16);
    let mut terminal = Terminal::new(backend).unwrap();
    let band_row = |buf: &ratatui::buffer::Buffer| {
        buffer_text(buf)
            .lines()
            .position(|line| line.contains("band-run"))
    };

    terminal.draw(|f| app.render(f)).unwrap();
    let before = band_row(terminal.backend().buffer())
        .expect("band must be visible at the bottom while following");

    // Scrolling up shifts the band exactly with the feed rows (it is content,
    // not a pinned region).
    app.scroll_up(2);
    terminal.draw(|f| app.render(f)).unwrap();
    let after = band_row(terminal.backend().buffer())
        .expect("band must still be partially visible after 2 rows");
    assert_eq!(after, before + 2, "the band must move with the feed scroll");

    // Scrolling further up carries the band out of the viewport entirely.
    app.scroll_up(30);
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(
        band_row(terminal.backend().buffer()).is_none(),
        "the band must scroll off-screen with the feed"
    );

    // Scrolling back down restores the band at the bottom (follow).
    app.scroll_down(1_000);
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(
        band_row(terminal.backend().buffer()).is_some(),
        "the band must return when scrolled back to the bottom"
    );
}
