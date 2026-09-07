use super::*;

// ── deferred fresh attach (issue #46) ──────────────────────────────────────────────────

/// Issue #46: with a pending deferred fresh attach (reused daemon, issue
/// #56), NO session is created at startup; the first submitted message
/// creates + selects the fresh session and then reaches the daemon under
/// the new session id. An idle TUI therefore leaves no empty conversation
/// behind.
#[tokio::test]
async fn first_submit_creates_deferred_fresh_session() {
    let (mut app, rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    // Simulate the startup wiring: `run_repl` arms the flag when it reuses
    // a live daemon without explicit session selection.
    app.pending_fresh_attach = true;
    assert_eq!(
        app.session_id, "sess-1",
        "starts on the daemon's current session"
    );

    let (drainer, seen) = drain_commands(rx);
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();

    app.set_input("hello");
    app.submit(&mut terminal).await.unwrap();

    // FakeSessionOps ids come from a counter: the first create is `sess-new-1`.
    assert_eq!(
        app.session_id, "sess-new-1",
        "first message must create and select the fresh session"
    );
    assert!(
        !app.pending_fresh_attach,
        "flag cleared after the first send"
    );
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|label| label == "Submit(hello)"),
        "the message must be forwarded to the daemon: {:?}",
        seen.lock().unwrap()
    );
    drainer.abort();
}

/// Issue #79: a reused-daemon fresh attach (no explicit resume) starts with
/// an EMPTY feed — `App::new` deliberately does not seed the previous
/// session's messages, so the stale conversation isn't shown. The fresh
/// session is created only on the first submitted message (issue #46).
#[tokio::test]
async fn fresh_attach_starts_with_empty_feed() {
    let (app, _rx, _ops) = test_app_with_sessions(&["sess-1"], true).await;
    assert!(
        app.feed.blocks().is_empty(),
        "fresh attach must not show the previous session's feed, got {}",
        app.feed.blocks().len()
    );
}

#[tokio::test]
async fn fresh_session_inherits_configured_model_and_thinking_defaults() {
    use theway_transport::transport::SessionOps;

    let seed = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        thinking_level: Some("high".into()),
        ..Default::default()
    };
    let (mut app, rx, ops, _config) =
        test_app_with_sessions_and_config(&["sess-1"], false, seed).await;
    app.pending_fresh_attach = true;
    let (drainer, seen) = drain_commands(rx);

    let id = app.ensure_fresh_session().await.unwrap();

    assert_eq!(app.session_id, id);
    assert!(ops.list().await.unwrap().iter().any(|s| s.session_id == id));
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|label| label == "SetModel(anthropic:claude-x)"),
        "a new session must inherit the configured default model: {seen:?}"
    );
    assert!(
        seen.iter().any(|label| label == "SetThinking(high)"),
        "a new session must inherit the configured thinking default: {seen:?}"
    );
    drainer.abort();
}

/// Issue #46: an explicit session selection (e.g. `/new`, `/resume`,
/// `/session switch` — all routed through `select_session`) cancels the
/// pending deferred fresh attach, so the next message goes to the chosen
/// session without creating anything extra.
#[tokio::test]
async fn explicit_session_selection_cancels_deferred_fresh_attach() {
    let (mut app, _rx, ops) = test_app_with_sessions(&["sess-1"], false).await;
    app.pending_fresh_attach = true;

    // `/new`-style explicit create + select: creates the session immediately
    // (deliberate user action) and clears the pending flag.
    app.ensure_fresh_session().await.unwrap();
    assert_eq!(app.session_id, "sess-new-1");
    assert!(!app.pending_fresh_attach);

    // A second message must not create yet another session.
    app.pending_fresh_attach = false;
    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    app.set_input("again");
    app.submit(&mut terminal).await.unwrap();
    assert_eq!(app.session_id, "sess-new-1");

    let (sessions, _current) = app.client.list_sessions().await.unwrap();
    assert_eq!(
        sessions.len(),
        2,
        "no extra session after the second message"
    );
    let _ = ops;
}

/// Issue #47: an idle TUI run deletes the SPAWNED daemon's startup session
/// on exit — no message ever reached it, so it must not remain as an empty
/// conversation.
#[tokio::test]
async fn reap_empty_auto_session_deletes_unmessaged_session() {
    let (mut app, _rx, ops) = test_app_with_sessions(&["sess-1"], false).await;
    let auto_id = ops.add_session("sess-auto");
    app.auto_session = Some(auto_id.clone());

    app.reap_empty_auto_session().await;

    let (sessions, _current) = app.client.list_sessions().await.unwrap();
    assert!(
        !sessions.iter().any(|s| s.session_id == auto_id),
        "unmessaged auto session must be reaped"
    );
    assert_eq!(sessions.len(), 1, "only the seed session remains");
}

/// Issue #47: a session that received a message is never reaped, even when
/// it was the daemon's startup session.
#[tokio::test]
async fn reap_empty_auto_session_keeps_messaged_session() {
    let (mut app, _rx, ops) = test_app_with_sessions(&["sess-1"], false).await;
    let auto_id = ops.add_session("sess-auto");
    app.auto_session = Some(auto_id.clone());
    app.messaged_sessions.insert(auto_id.clone());

    app.reap_empty_auto_session().await;

    let (sessions, _current) = app.client.list_sessions().await.unwrap();
    assert!(
        sessions.iter().any(|s| s.session_id == auto_id),
        "messaged session must survive"
    );
}

/// Issue #47: without an auto session (reused daemon or explicit selection)
/// the reap is a no-op.
#[tokio::test]
async fn reap_empty_auto_session_noop_without_auto_session() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;

    app.reap_empty_auto_session().await;

    let (sessions, _current) = app.client.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
}

/// Issue #95: with a bare "/" in the composer the skill catalog entries must
/// sit at the front of the completion popup — the alphabetical ordering
/// would bury them below the popup's visible rows.
#[tokio::test]
async fn bare_slash_surfaces_skill_catalog_first() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    app.latest.sidebar.skills = theway_transport::wire::WireSkillsSnapshot {
        total: 2,
        enabled: 2,
        disabled: 0,
        builtin: 0,
        user: 2,
        project: 0,
        items: vec![
            WireSkillSnapshot {
                name: "release-checklist".into(),
                source: "user".into(),
                file_path: "/tmp/skills/release-checklist".into(),
                enabled: true,
            },
            WireSkillSnapshot {
                name: "zebra-skill".into(),
                source: "user".into(),
                file_path: "/tmp/skills/zebra-skill".into(),
                enabled: true,
            },
        ],
    };

    app.set_input("/");
    assert!(
        !app.completions.is_empty(),
        "a bare slash must produce completions"
    );
    let skill_entries: Vec<&String> = app
        .completions
        .iter()
        .filter(|entry| **entry == "/release-checklist" || **entry == "/zebra-skill")
        .collect();
    assert_eq!(skill_entries.len(), 2, "both skills must be listed once");
    assert!(
        !app.completions
            .iter()
            .any(|entry| entry.starts_with("/skill::")),
        "unique shortcuts must not duplicate as skill:: entries: {:?}",
        app.completions
    );
    let first_two: Vec<&String> = app.completions.iter().take(2).collect();
    assert!(
        first_two
            .iter()
            .all(|entry| entry.as_str() == "/release-checklist" || entry.as_str() == "/zebra-skill"),
        "skill entries must come first for a bare slash: {:?}",
        app.completions
    );
    assert!(
        app.completions.contains(&"/clear".to_string()),
        "regular commands must still be reachable after the skill entries"
    );

    // A prefixed query keeps plain alphabetical matching.
    app.set_input("/cl");
    assert!(app.completions.contains(&"/clear".to_string()));
    assert!(
        app.completions.iter().all(|entry| entry.starts_with("/cl")),
        "prefixed queries must not reorder: {:?}",
        app.completions
    );
}

/// Issue #97: an eager fresh attach produces the new session id immediately
/// — `ensure_fresh_session` returns it and the App points at it (panel/feed/
/// stream never reference the daemon's previous session).
#[tokio::test]
async fn eager_fresh_attach_returns_and_selects_the_new_session() {
    use theway_transport::transport::SessionOps;

    let (mut app, _rx, ops) = test_app_with_sessions(&["sess-1"], true).await;
    assert_eq!(app.session_id, "sess-1", "fixture seeds the previous id");

    let id = app.ensure_fresh_session().await.unwrap();
    assert_ne!(id, "sess-1", "fresh attach must create a brand-new session");
    assert_eq!(app.session_id, id, "the App must point at the new session");
    assert!(
        ops.list().await.unwrap().iter().any(|s| s.session_id == id),
        "the new session must exist server-side"
    );
    assert!(!app.pending_fresh_attach);

    // Marking it as the auto session + idle exit reaps it (issue #47).
    app.set_auto_session(id.clone());
    app.reap_empty_auto_session().await;
    assert!(
        ops.list().await.unwrap().iter().all(|s| s.session_id != id),
        "an untouched fresh session must be reaped on idle exit"
    );
}

/// Issue #99: a daemon RPC that never answers must fail fast instead of
/// hanging the UI — the helper bounds the await and names the call.
#[tokio::test]
async fn daemon_call_times_out_instead_of_hanging() {
    let start = std::time::Instant::now();
    let result = crate::ui::daemon_call_with(
        std::time::Duration::from_millis(200),
        "get_snapshot",
        std::future::pending::<anyhow::Result<()>>(),
    )
    .await;
    assert!(result.is_err(), "a hung call must surface an error");
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("get_snapshot"),
        "the error must name the call: {message}"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "the timeout must be respected (took {:?})",
        start.elapsed()
    );
}
