// ── path context (issue #68) ─────────────────────────────────────────

fn startup_path_context() -> WirePathContext {
    WirePathContext {
        home: "/home/dev".into(),
        base: "/home/dev/.theway".into(),
        work_dir: "/home/dev/projects/theway".into(),
        skills_dirs: vec!["/home/dev/.agents/skills".into()],
    }
}

#[tokio::test]
async fn client_get_path_context_returns_startup_paths_and_skill_dirs() {
    let ctx = startup_path_context();
    let (mut client, _command_rx, _snapshot_tx) =
        client_and_server_with_path_context(ctx.clone()).await;

    // Read-only snapshot: startup home/base/work_dir + the initial skills_dirs.
    let got = client.get_path_context().await.unwrap();
    assert_eq!(got, ctx);
}

#[tokio::test]
async fn client_set_skill_dirs_queues_command_and_updates_path_context() {
    let ctx = startup_path_context();
    let (mut client, mut command_rx, _snapshot_tx) =
        client_and_server_with_path_context(ctx.clone()).await;

    let accepted = client
        .set_skill_dirs(&["/skills/a".to_string(), "/skills/b".to_string()])
        .await
        .unwrap();
    assert!(accepted);

    // The event loop receives WireCommand::SetSkillDirs for the authoritative
    // apply (extras replacement + hot-reload).
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetSkillDirs { dirs } => {
            assert_eq!(dirs, vec!["/skills/a", "/skills/b"])
        }
        other => panic!("unexpected command: {other:?}"),
    }

    // Optimistic update: the follow-up read reflects the new dirs while
    // home/base/work_dir stay startup-fixed.
    let got = client.get_path_context().await.unwrap();
    assert_eq!(got.skills_dirs, vec!["/skills/a", "/skills/b"]);
    assert_eq!(got.home, ctx.home);
    assert_eq!(got.base, ctx.base);
    assert_eq!(got.work_dir, ctx.work_dir);

    // Clearing: an empty list is a valid update.
    let accepted = client.set_skill_dirs(&[]).await.unwrap();
    assert!(accepted);
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetSkillDirs { dirs } => assert!(dirs.is_empty()),
        other => panic!("unexpected command: {other:?}"),
    }
    let got = client.get_path_context().await.unwrap();
    assert!(got.skills_dirs.is_empty());
}

#[tokio::test]
async fn client_stream_events_receives_snapshot_frames() {
    let (mut client, _command_rx, snapshot_tx) = client_and_server().await;
    let mut stream = client.stream_events().await.unwrap();
    // The fixture publishes on demand (no event loop in tests).
    snapshot_tx.send(fixture_status("streamed")).unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for frame")
        .expect("stream ended")
        .unwrap();
    match frame.payload {
        Some(crate::proto::theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.session_id, "sess-1");
            assert_eq!(state.feed_lines, vec!["streamed"]);
        }
        other => panic!("expected snapshot payload, got {other:?}"),
    }
}

#[tokio::test]
async fn client_connect_to_dead_port_fails_promptly() {
    // Bind a listener, note the port, drop it — nothing listens anymore.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let err = GrpcClient::connect(&addr).await.unwrap_err().to_string();
    assert!(err.contains("connect"), "{err}");
}

#[tokio::test]
async fn probe_reports_live_daemon() {
    let (client, _command_rx, _snapshot_tx) = client_and_server().await;
    let addr = client.addr().to_string();
    let state = probe(&addr, Duration::from_secs(2)).await.unwrap();
    assert_eq!(state.session_id, "sess-1");
}

#[tokio::test]
async fn probe_fails_on_dead_port() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    assert!(probe(&addr, Duration::from_millis(300)).await.is_err());
}

// ── port-file discovery ───────────────────────────────────────────

/// THEWAY_DIR is process-global; all port-file tests serialize on this lock.
static THEWAY_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_theway_dir(dir: &std::path::Path) {
    // SAFETY: tests are single-threaded per test and serialized on
    // THEWAY_DIR_LOCK; no other thread reads THEWAY_DIR concurrently.
    unsafe { std::env::set_var("THEWAY_DIR", dir) };
}

fn clear_theway_dir() {
    // SAFETY: see with_theway_dir.
    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[test]
fn port_file_round_trips_the_bound_port() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    assert_eq!(read_port_file(&cwd).unwrap(), None, "no port file yet");
    std::fs::write(port_file_path(&cwd), "44777 1234").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 44777, pid: Some(1234) })
    );
    std::fs::write(port_file_path(&cwd), "0 1").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 0, pid: Some(1) })
    );
    // Pre-pid format (single token) still parses, pid unknown.
    std::fs::write(port_file_path(&cwd), "44777").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 44777, pid: None })
    );
    drop(dir);
    clear_theway_dir();
}

#[test]
fn port_file_with_garbage_is_an_error() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();
    std::fs::write(port_file_path(&cwd), "not-a-port").unwrap();
    assert!(read_port_file(&cwd).is_err());
    std::fs::write(port_file_path(&cwd), "44777 not-a-pid").unwrap();
    assert!(read_port_file(&cwd).is_err());
    drop(dir);
    clear_theway_dir();
}

#[test]
fn candidate_addrs_prefers_live_port_file_then_default() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    // No port file → default only.
    assert_eq!(
        candidate_addrs(&cwd),
        vec![format!("127.0.0.1:{DEFAULT_PORT}")]
    );

    // Entry whose pid is dead → skipped, default only (the stale-entry case
    // that used to break cold starts). Linux-only: outside Linux pid_alive
    // cannot verify, so the entry is probed as a best effort.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", u32::MAX)).unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            candidate_addrs(&cwd),
            vec![format!("127.0.0.1:{DEFAULT_PORT}")]
        );
    }

    // Entry whose pid is alive (ours) → port-file address first, default second.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", std::process::id())).unwrap();
    assert_eq!(
        candidate_addrs(&cwd),
        vec!["127.0.0.1:43001".to_string(), format!("127.0.0.1:{DEFAULT_PORT}")]
    );
    drop(dir);
    clear_theway_dir();
}

#[test]
fn remove_port_file_removes_only_the_owners_entry() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    // Own entry → removed.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", std::process::id())).unwrap();
    remove_port_file_if_owner(&cwd, std::process::id());
    assert_eq!(read_port_file(&cwd).unwrap(), None);

    // Foreign entry (a successor daemon) → untouched.
    std::fs::write(port_file_path(&cwd), "43001 424242").unwrap();
    remove_port_file_if_owner(&cwd, std::process::id());
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 43001, pid: Some(424242) })
    );
    drop(dir);
    clear_theway_dir();
}
