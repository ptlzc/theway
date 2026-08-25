//! Tests for the daemon-side hook command executor.
//!
//! The core hook runner only defines an injected seam for side effects; these
//! tests pin the daemon implementation: it routes hook commands through the
//! shared process-group-kill primitive (`tools::exec`) with millisecond
//! timeouts and env plumbing, and it kills the whole shell tree on
//! timeout/cancel. The kill-semantics tests moved here from the core hook unit
//! tests when the core inline implementation was replaced by the seam.
//!
//! The commands are `sh -c` shell snippets — the whole file is gated on unix.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use theway_daemon::hook_executors::daemon_executors;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// Assert that no process whose command line matches `marker` survived the run.
/// (The pgrep check itself is a test probe, not a daemon primitive.)
async fn assert_no_survivors(marker: &str) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    let pgrep = tokio::process::Command::new("pgrep")
        .arg("-f")
        .arg(marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = pgrep {
        let mut buf = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = s.read_to_string(&mut buf).await;
        }
        let _ = child.wait().await;
        assert!(
            buf.trim().is_empty(),
            "found surviving descendant matching {marker:?}: pids={buf}"
        );
    }
}

/// The executor runs the command with the injected env and cwd and captures
/// stdout/stderr, exiting successfully.
#[tokio::test]
async fn hook_command_executor_runs_with_env_and_cwd() {
    let executors = daemon_executors();
    let exec = executors.command.expect("command executor injected");
    let dir = tempfile::tempdir().unwrap();
    let mut envs = BTreeMap::new();
    envs.insert("HOOK_EXEC_TEST_VAR".into(), "hook-exec-env-ok".into());
    let out = exec(
        "printf '%s' \"$HOOK_EXEC_TEST_VAR\"".into(),
        dir.path().to_path_buf(),
        envs,
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .expect("command should succeed");
    assert_eq!(out.stdout, "hook-exec-env-ok");
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
}

/// A non-zero exit surfaces as the hook error contract ("command exited
/// <code>: <stderr>"), not as Ok.
#[tokio::test]
async fn hook_command_executor_nonzero_exit_is_error() {
    let executors = daemon_executors();
    let exec = executors.command.expect("command executor injected");
    let err = exec(
        "echo boom >&2; exit 7".into(),
        std::env::current_dir().unwrap(),
        BTreeMap::new(),
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .expect_err("non-zero exit must be an error");
    let msg = err.to_string();
    assert!(msg.contains("command exited 7"), "{msg}");
    assert!(msg.contains("boom"), "{msg}");
}

/// A hook command that exceeds `timeout_ms` must be killed, including any
/// descendant process the shell backgrounded — millisecond timeouts and the
/// whole-tree kill are the executor's job (via the shared primitive).
#[tokio::test]
async fn hook_command_timeout_kills_descendant_process() {
    let executors = daemon_executors();
    let exec = executors.command.expect("command executor injected");
    let marker = "theway-hook-exec-timeout-test-mkr-z3x8c2";
    let command = format!("(sleep 30 && echo {marker}) & wait");
    let started = std::time::Instant::now();
    let err = exec(
        command,
        std::env::current_dir().unwrap(),
        BTreeMap::new(),
        Duration::from_millis(100),
        CancellationToken::new(),
    )
    .await
    .expect_err("timeout must be an error");
    assert!(err.to_string().contains("timed out after 100ms"), "{err}");
    assert!(
        started.elapsed().as_secs() < 5,
        "hook timeout path took {:?}; descendant kill did not happen in time",
        started.elapsed()
    );
    assert_no_survivors(marker).await;
}

/// Cancellation token tripped mid-hook must kill the whole shell tree,
/// mirroring the timeout path.
#[tokio::test]
async fn hook_command_cancellation_kills_descendant_process() {
    let executors = daemon_executors();
    let exec = executors.command.expect("command executor injected");
    let marker = "theway-hook-exec-cancel-test-mkr-z3x9d4";
    let command = format!("(sleep 30 && echo {marker}) & wait");

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_clone.cancel();
    });

    let started = std::time::Instant::now();
    let err = exec(
        command,
        std::env::current_dir().unwrap(),
        BTreeMap::new(),
        Duration::from_secs(30),
        cancel,
    )
    .await
    .expect_err("cancellation must be an error");
    assert!(err.to_string().contains("cancelled"), "{err}");
    assert!(
        started.elapsed().as_secs() < 5,
        "hook cancel path took {:?}; descendant kill did not happen in time",
        started.elapsed()
    );
    assert_no_survivors(marker).await;
}

/// Spawn a one-shot HTTP server that captures the request text and responds with
/// `status` + `body`.
async fn spawn_http_responder(
    status: &'static str,
    body: &'static str,
) -> (
    String,
    tokio::sync::oneshot::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 4096];
        let n = socket.read(&mut buf).await.unwrap_or(0);
        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
        let body = body.as_bytes();
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(body).await;
        let _ = socket.shutdown().await;
    });
    (addr.to_string(), rx, handle)
}

/// The webhook executor posts the payload, sends custom headers, and accepts a
/// successful response.
#[tokio::test]
async fn hook_webhook_posts_payload_headers_and_succeeds() {
    let executors = daemon_executors();
    let send = executors.webhook.expect("webhook executor injected");
    let (addr, rx, handle) = spawn_http_responder("204 No Content", "").await;
    let mut headers = BTreeMap::new();
    headers.insert("X-Test-Header".into(), "hook-header-value".into());

    send(
        format!("http://{addr}/hook"),
        r#"{"ok":true}"#.into(),
        headers,
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .expect("webhook should succeed");

    let request = rx.await.expect("server captured request");
    assert!(request.starts_with("POST /hook HTTP/1.1"), "{request}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-test-header: hook-header-value"),
        "{request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("user-agent: theway/"),
        "{request}"
    );
    handle.await.unwrap();
}

/// Non-2xx webhook responses are errors that include the status and a bounded body.
#[tokio::test]
async fn hook_webhook_non_success_status_is_error() {
    let executors = daemon_executors();
    let send = executors.webhook.expect("webhook executor injected");
    let (addr, _rx, handle) =
        spawn_http_responder("500 Internal Server Error", "boom details").await;

    let err = send(
        format!("http://{addr}/hook"),
        "{}".into(),
        BTreeMap::new(),
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .expect_err("non-success must be an error");
    let msg = err.to_string();
    assert!(msg.contains("webhook status 500"), "{msg}");
    assert!(msg.contains("boom details"), "{msg}");
    handle.await.unwrap();
}

/// Cancellation mid-webhook aborts the request and surfaces the hook error contract.
#[tokio::test]
async fn hook_webhook_cancellation_is_error() {
    use tokio::net::TcpListener;

    let executors = daemon_executors();
    let send = executors.webhook.expect("webhook executor injected");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0_u8; 4096];
        let _ = socket.read(&mut buf).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(socket);
    });
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_clone.cancel();
    });

    let err = send(
        format!("http://{addr}/hook"),
        "{}".into(),
        BTreeMap::new(),
        Duration::from_secs(30),
        cancel,
    )
    .await
    .expect_err("cancelled webhook must be an error");
    assert!(err.to_string().contains("cancelled"), "{err}");
    handle.abort();
}

/// A command killed by a signal (no normal exit code) uses the explicit
/// "failed to produce an exit code" hook error contract.
#[tokio::test]
async fn hook_command_missing_exit_code_is_error() {
    let executors = daemon_executors();
    let exec = executors.command.expect("command executor injected");
    let err = exec(
        "kill -9 $$".into(),
        std::env::current_dir().unwrap(),
        BTreeMap::new(),
        Duration::from_secs(5),
        CancellationToken::new(),
    )
    .await
    .expect_err("signal-killed command must be an error");
    assert!(
        err.to_string().contains("failed to produce an exit code"),
        "{err}"
    );
}
