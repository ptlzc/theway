//! Daemon-client e2e (openspec tui-connect-daemon 1.3): spawn the real `thewayd`
//! binary and drive it through the transport `GrpcClient` — the exact path the
//! TUI will use. Covers readiness (2.1: get_state works immediately after the
//! port file appears), command round-trips (send_message / switch_session /
//! stream frames) and multi-client fan-out (2.2) against a live daemon.

use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt as _;
use theway_transport::client::{probe, wait_ready, GrpcClient};
use theway_transport::proto::theway_grpc::{self, stream_frame};

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
    let binary = env!("CARGO_BIN_EXE_thewayd");
    let mut child = Command::new(binary)
        .arg("--port")
        .arg("0")
        .arg("--cwd")
        .arg(dir)
        // Credential-less start: no provider key in tests; the daemon logs a
        // warning and starts anyway (turns fail until a key is configured).
        .env("THEWAY_DIR", dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn thewayd");
    // Give the daemon a moment to exec, then wait for the port file + readiness.
    let addr = tokio::time::timeout(
        Duration::from_secs(20),
        wait_ready(Duration::from_secs(20)),
    )
    .await
    .expect("daemon never became ready")
    .expect("wait_ready failed");
    (DaemonGuard { child }, addr)
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
    assert_eq!(state.cwd, dir.path().display().to_string());

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
    assert!(err.is_err(), "probe against a dead daemon must fail: {err:?}");

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
