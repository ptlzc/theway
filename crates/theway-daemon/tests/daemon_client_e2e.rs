//! Daemon-client e2e (openspec tui-connect-daemon 1.3): spawn the real `thewayd`
//! binary and drive it through the transport `GrpcClient` — the exact path the
//! TUI will use. Covers readiness (2.1: get_state works immediately after the
//! port file appears), command round-trips (send_message / switch_session /
//! stream frames) and multi-client fan-out (2.2) against a live daemon.

use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt as _;
use theway_transport::client::{GrpcClient, port_file_path, probe, wait_ready};
use theway_transport::proto::theway_grpc::stream_frame;

/// THEWAY_DIR is process-global; all daemon-spawning tests serialize here.
static DAEMON_E2E_LOCK: Mutex<()> = Mutex::new(());

/// Spawned `thewayd` guard: kills the child on drop so tests never leak daemons.
struct DaemonGuard {
    child: Child,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a daemon in a temp THEWAY_DIR with `--port 0` (random, published to the
/// port file) and `--cwd` pointing at the temp dir. Returns the guard + the
/// ready client address (from `wait_ready`, exercising the port-file discovery).
async fn spawn_daemon(dir: &std::path::Path) -> (DaemonGuard, String) {
    spawn_daemon_with(dir, |cmd| cmd).await
}

/// [`spawn_daemon`] with extra launch arguments (e.g. repeatable
/// `--skills-dir` extras for the path-context e2e, issue #68).
async fn spawn_daemon_with(
    dir: &std::path::Path,
    customize: impl FnOnce(&mut Command) -> &mut Command,
) -> (DaemonGuard, String) {
    let binary = env!("CARGO_BIN_EXE_thewayd");
    let mut command = Command::new(binary);
    customize(
        command
            .arg("--port")
            .arg("0")
            .arg("--cwd")
            .arg(dir)
            // Credential-less start: no provider key in tests; the daemon logs
            // a warning and starts anyway (turns fail until a key is set).
            .env("THEWAY_DIR", dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    );
    let child = command.spawn().expect("spawn thewayd");
    let child_pid = child.id();
    // Give the daemon a moment to exec, then wait for the port file + readiness.
    let addr = tokio::time::timeout(
        Duration::from_secs(20),
        wait_ready(Duration::from_secs(20), dir, child_pid),
    )
    .await
    .expect("daemon never became ready")
    .expect("wait_ready failed");
    (DaemonGuard { child }, addr)
}

#[tokio::test]
async fn wait_ready_ignores_stale_entry_from_a_dead_daemon() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK; no other test touches THEWAY_DIR.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    // A leftover entry from a dead daemon (the cold-start failure mode: the
    // file exists, so wait_ready must NOT accept it and must wait for the
    // freshly spawned child's own entry).
    std::fs::write(port_file_path(dir.path()), format!("1 {}", u32::MAX)).unwrap();

    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    assert_ne!(
        addr, "127.0.0.1:1",
        "wait_ready accepted the stale port instead of the spawned daemon's"
    );
    let mut client = GrpcClient::connect(&addr).await.unwrap();
    let state = client.get_state().await.unwrap();
    let expected_cwd =
        std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
    assert_eq!(state.cwd, expected_cwd.display().to_string());

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[tokio::test]
async fn spawned_daemon_serves_get_state_immediately() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK; no other test touches THEWAY_DIR.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    let mut client = GrpcClient::connect(&addr).await.unwrap();

    // 2.1 readiness: get_state works right after the port file appears (the
    // daemon writes the file on bind, before serve — but a probe must succeed
    // immediately, not hang).
    let state = tokio::time::timeout(Duration::from_secs(5), client.get_state())
        .await
        .expect("get_state hung after readiness")
        .unwrap();
    assert!(!state.session_id.is_empty(), "daemon created a session");
    let expected_cwd =
        std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
    assert_eq!(state.cwd, expected_cwd.display().to_string());

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[tokio::test]
async fn client_round_trip_against_spawned_daemon() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    let mut client = GrpcClient::connect(&addr).await.unwrap();
    let state = client.get_state().await.unwrap();
    let session_id = state.session_id.clone();

    // send_message → accepted (no credential, so the turn errors server-side,
    // but the command is queued and a snapshot lands on the stream).
    let accepted = client
        .send_message("hello from the client".into(), vec![], false)
        .await
        .unwrap();
    assert!(accepted, "send_message accepted");

    // stream frames arrive (snapshot after the command was processed).
    let mut stream = client.stream_events().await.unwrap();
    let mut saw_snapshot = false;
    for _ in 0..4 {
        let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("timed out waiting for stream frame")
            .expect("stream ended")
            .expect("stream frame error");
        if let Some(stream_frame::Payload::Snapshot(snapshot)) = frame.payload {
            saw_snapshot = true;
            assert_eq!(snapshot.session_id, session_id);
            break;
        }
    }
    assert!(saw_snapshot, "stream carried a snapshot frame");

    // switch_session to an unknown id → NOT_FOUND, daemon stays put.
    let err = client
        .switch_session("no-such-session")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no session matches"), "{err}");
    let state = client.get_state().await.unwrap();
    assert_eq!(state.session_id, session_id, "unknown switch did not move");

    // switch_session to the current session → accepted (idempotent rebind).
    assert!(client.switch_session(&session_id).await.unwrap());

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[tokio::test]
async fn two_clients_both_receive_frames_from_spawned_daemon() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    let mut client_a = GrpcClient::connect(&addr).await.unwrap();
    let mut client_b = GrpcClient::connect(&addr).await.unwrap();
    let mut stream_a = client_a.stream_events().await.unwrap();
    let mut stream_b = client_b.stream_events().await.unwrap();

    // A command from either client publishes a snapshot to both subscribers.
    client_b
        .send_message("broadcast test".into(), vec![], false)
        .await
        .unwrap();

    for (label, stream) in [("a", &mut stream_a), ("b", &mut stream_b)] {
        let mut saw_snapshot = false;
        for _ in 0..4 {
            let frame = tokio::time::timeout(Duration::from_secs(5), stream.next())
                .await
                .expect("timed out")
                .expect("stream ended")
                .expect("stream frame error");
            if let Some(stream_frame::Payload::Snapshot(_)) = frame.payload {
                saw_snapshot = true;
                break;
            }
        }
        assert!(saw_snapshot, "client {label} received a snapshot frame");
    }

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[tokio::test]
async fn dead_daemon_probe_fails_promptly() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    // Start a daemon, let it publish its port, then kill it — the same port now
    // must fail the probe quickly (no hang, no stale readiness).
    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    drop(_daemon); // kills the child
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = probe(&addr, Duration::from_millis(500)).await;
    assert!(
        err.is_err(),
        "probe against a dead daemon must fail: {err:?}"
    );

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[tokio::test]
async fn grpc_surface_has_health_check() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    let mut health = theway_transport::proto::health::health_client::HealthClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();
    let response = health
        .check(theway_transport::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        response.status,
        theway_transport::proto::health::health_check_response::ServingStatus::Serving as i32
    );

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[tokio::test]
async fn list_sessions_marks_current_after_spawn() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    let (_daemon, addr) = spawn_daemon(dir.path()).await;
    let mut client = GrpcClient::connect(&addr).await.unwrap();
    let (sessions, current) = client.list_sessions().await.unwrap();
    assert!(!current.is_empty(), "daemon has a current session");
    assert!(
        sessions.iter().any(|s| s.session_id == current),
        "current session listed: {sessions:?}"
    );

    unsafe { std::env::remove_var("THEWAY_DIR") };
}

/// Issue #68: the gRPC path-context surface against a live daemon. The
/// startup `--skills-dir` extras are served by `GetPathContext`; `SetSkillDirs`
/// replaces them dynamically and the change is visible to a follow-up read.
#[tokio::test]
async fn path_context_round_trip_against_spawned_daemon() {
    let _guard = DAEMON_E2E_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized on DAEMON_E2E_LOCK.
    unsafe { std::env::set_var("THEWAY_DIR", dir.path()) };

    let startup_extra = dir.path().join("extra-skills");
    std::fs::create_dir_all(&startup_extra).unwrap();
    let startup_extra_arg = startup_extra.clone();

    let (_daemon, addr) = spawn_daemon_with(dir.path(), move |cmd| {
        cmd.arg("--skills-dir").arg(&startup_extra_arg)
    })
    .await;
    let mut client = GrpcClient::connect(&addr).await.unwrap();

    // GetPathContext: startup home/base/work_dir + the CLI-supplied extras.
    let ctx = client.get_path_context().await.unwrap();
    let expected_work_dir =
        std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
    assert_eq!(ctx.work_dir, expected_work_dir.display().to_string());
    assert_eq!(ctx.base, dir.path().display().to_string());
    assert!(!ctx.home.is_empty(), "home resolved at startup");
    assert_eq!(
        ctx.skills_dirs,
        vec![startup_extra.display().to_string()],
        "startup --skills-dir extras served by GetPathContext"
    );

    // SetSkillDirs: dynamically replace the extras; a follow-up GetPathContext
    // reflects the new dirs while home/base/work_dir stay startup-fixed.
    let replacement = dir.path().join("replacement-skills");
    std::fs::create_dir_all(&replacement).unwrap();
    let accepted = client
        .set_skill_dirs(&[replacement.display().to_string()])
        .await
        .unwrap();
    assert!(accepted, "SetSkillDirs command queued");

    let ctx = client.get_path_context().await.unwrap();
    assert_eq!(
        ctx.skills_dirs,
        vec![replacement.display().to_string()],
        "SetSkillDirs update visible to GetPathContext"
    );
    assert_eq!(ctx.work_dir, expected_work_dir.display().to_string());
    assert_eq!(ctx.base, dir.path().display().to_string());

    unsafe { std::env::remove_var("THEWAY_DIR") };
}
