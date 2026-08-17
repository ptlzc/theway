//! Tests for `bash` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

/// Spawn a long-running command, hit the timeout, and assert the child process is
/// gone afterwards. The previous implementation marked the result `[timed out]` but
/// left `sh -c sleep ...` running in the background.
///
/// Uses a unique sleep duration (`sleep 47383`) so the `pgrep -f` check can scope to
/// this test only — `cargo test` runs tests in parallel, and a sibling test using
/// plain `sleep 60` would otherwise collide. Pick a value that's:
/// 1. larger than any plausible test wall-clock, so the kill path is the only exit
/// 2. unique across this file's tests
#[tokio::test]
async fn timeout_kills_child_process() {
    const SLEEP_SECS: &str = "47383";
    let tool = BashTool;
    let started = Instant::now();
    let result = tool
        .execute(
            "t1",
            json!({ "command": format!("sleep {SLEEP_SECS}"), "timeout": 1 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("bash tool execute should not error on timeout");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "timeout path took {elapsed:?}; child kill did not happen in time"
    );
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(
        text.contains("[timed out after 1s]"),
        "expected timeout marker in output, got: {text}"
    );
    assert!(text.contains("[exit -1]"));

    // Give the OS a beat to reap the killed process group.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify no `sleep 47383` process survived. `pgrep -f` matches the full argv,
    // including the shell wrapper if any sibling test happened to spawn one — the
    // unique duration scopes the check to this test.
    let pgrep = tokio::process::Command::new("pgrep")
        .arg("-f")
        .arg(format!("sleep {SLEEP_SECS}"))
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
            "found surviving `sleep {SLEEP_SECS}` process(es) after timeout: pids={buf}"
        );
    }
}

/// Timeout must kill not just the direct `sh -c` child but any descendants the shell
/// spawned (background jobs, detached processes). The previous implementation killed
/// only the direct child, so `(sleep 60) & wait` left `sleep 60` running after the
/// tool returned. We solve it the same way `NativeEnv::exec` does (PR #40): run the
/// child in its own process group via `setsid` and `killpg` the whole group on
/// timeout. Unix-only because `setsid` / `killpg` are Unix primitives.
#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_descendant_processes() {
    let tool = BashTool;
    // The pattern `(cmd) & wait` is the canonical shell-detached-child case: the
    // background job inherits the shell's process group, so killing only the shell
    // would leave the `sleep` running. Use a marker arg unique to this test so the
    // `pgrep` check doesn't false-positive against other tests' sleeps.
    let marker = "bash-tool-desc-kill-marker-7f3a9c";
    let cmd = format!("(sleep 60 && echo {marker}) & wait");
    let started = Instant::now();
    let _result = tool
        .execute(
            "tdesc",
            json!({ "command": cmd, "timeout": 1 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("bash tool execute should not error on timeout");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "timeout path took {elapsed:?}; descendant kill did not happen in time"
    );

    // Give the OS a beat to actually reap the descendant tree.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // No process should match the marker after teardown. We grep for the literal
    // command string the shell would have launched.
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
            "found surviving descendant process(es) matching {marker:?}: pids={buf}"
        );
    }
}

/// Cancel via the token mid-run. Same expectation as timeout: child is killed, output
/// includes the `[aborted]` marker, and no zombie remains. Distinct sleep duration
/// from `timeout_kills_child_process` so `pgrep` checks across the file don't collide
/// when `cargo test` runs in parallel.
#[tokio::test]
async fn cancellation_kills_child_process() {
    const SLEEP_SECS: &str = "47384";
    let tool = BashTool;
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    // Trip the token 200ms in.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_clone.cancel();
    });
    let started = Instant::now();
    let result = tool
        .execute(
            "t2",
            json!({ "command": format!("sleep {SLEEP_SECS}") }),
            cancel,
            None,
        )
        .await
        .expect("bash tool should not error on cancellation");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "cancellation path took {elapsed:?}; child kill did not happen in time"
    );
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(
        text.contains("[aborted]"),
        "expected aborted marker in output, got: {text}"
    );
    assert!(text.contains("[exit -1]"));
}

/// Cancel must tear down the whole tree exactly like timeout does — a backgrounded
/// descendant (`(sleep ...) & wait`) must not survive the agent's Ctrl-C. Mirrors
/// `timeout_kills_descendant_processes` through the cancellation-token path instead.
#[cfg(unix)]
#[tokio::test]
async fn cancellation_kills_descendant_processes() {
    let tool = BashTool;
    // Unique marker so the `pgrep` check can't false-positive against sibling tests.
    let marker = "bash-tool-cancel-desc-kill-marker-b21e4d";
    let cmd = format!("(sleep 60 && echo {marker}) & wait");
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    // Trip the token once the backgrounded subshell is up.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel_clone.cancel();
    });
    let started = Instant::now();
    let _result = tool
        .execute("cdesc", json!({ "command": cmd }), cancel, None)
        .await
        .expect("bash tool execute should not error on cancellation");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "cancellation path took {elapsed:?}; descendant kill did not happen in time"
    );

    // Give the OS a beat to actually reap the descendant tree.
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
            "found surviving descendant process(es) matching {marker:?} after cancel: pids={buf}"
        );
    }
}

/// A command that writes a lot to stderr while stdout is also active must not deadlock
/// the tool. The previous sequential drain (`read stdout → read stderr`) would block
/// on stdout while the child blocked writing to a full stderr pipe.
#[tokio::test]
async fn high_volume_stderr_does_not_deadlock_stdout() {
    let tool = BashTool;
    // Emit 256 KiB on each stream so both pipes saturate. The previous implementation
    // would hang forever waiting for stdout to EOF while the child blocked writing
    // stderr.
    let command = "yes hello | head -c 262144 ; yes world | head -c 262144 1>&2";
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        tool.execute(
            "t3",
            json!({ "command": command, "timeout": 10 }),
            CancellationToken::new(),
            None,
        ),
    )
    .await
    .expect("bash tool must not hang on high-volume stderr")
    .expect("execute returned error");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 8,
        "high-volume stderr drain took {elapsed:?}; sequential drain regression?"
    );
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    // Exit 0 expected since the command itself completes.
    assert!(
        text.contains("[exit 0]"),
        "expected clean exit, got: {text}"
    );
    // stderr marker should appear because we wrote a lot to it.
    assert!(text.contains("[stderr]"));
}

/// Sanity: a small, fast command still works the same as before.
#[tokio::test]
async fn ok_path_still_works() {
    let tool = BashTool;
    let r = tool
        .execute(
            "t4",
            json!({ "command": "echo hello" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("simple echo should not error");
    let text = match &r.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(text.contains("hello"));
    assert!(text.contains("[exit 0]"));
}

/// Concurrency check: spawn 4 short bash invocations from the same task tree to make
/// sure the new drain pattern doesn't accidentally serialize them. (Each completes in
/// well under 1 second; if there's a hidden global lock the wall time blows up.)
#[tokio::test]
async fn concurrent_invocations_do_not_serialize() {
    let tool = Arc::new(BashTool);
    let started = Instant::now();
    let mut handles = Vec::new();
    for i in 0..4 {
        let tool = tool.clone();
        handles.push(tokio::spawn(async move {
            tool.execute(
                &format!("c{i}"),
                json!({ "command": "sleep 0.3 && echo done" }),
                CancellationToken::new(),
                None,
            )
            .await
        }));
    }
    for h in handles {
        h.await.expect("task join").expect("execute should succeed");
    }
    let elapsed = started.elapsed();
    // 4 × 0.3s in parallel ≈ 0.3-0.6s; serialized would be ≥1.2s. Allow 1.5s for CI
    // jitter.
    assert!(
        elapsed.as_secs_f64() < 1.5,
        "concurrent bash calls serialized? elapsed = {elapsed:?}"
    );
}

#[test]
fn resolve_timeout_defaults_and_override() {
    // No `timeout` param → the default kicks in (runaway commands must not run unbounded).
    assert_eq!(resolve_timeout(&json!({})), 60);
    assert_eq!(resolve_timeout(&json!({ "command": "x" })), 60);
    // Explicit param wins.
    assert_eq!(resolve_timeout(&json!({ "timeout": 7 })), 7);
    assert_eq!(resolve_timeout(&json!({ "timeout": 0 })), 0);
}

#[tokio::test]
async fn bash_run_in_background_returns_shell_id() {
    // Relative on purpose: this module also compiles inside integration tests that pull
    // `tools/` in via `#[path]` at a different crate-root depth (server tests/tools.rs).
    let tool = BashTool;
    let result = tool
        .execute(
            "b1",
            json!({ "command": "echo bg", "run_in_background": true }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("bash");
    let text = text_of(&result);
    assert!(
        text.contains("background shell started: shell-"),
        "got: {text}"
    );

    // Foreground path is untouched.
    let fg = tool
        .execute(
            "b2",
            json!({ "command": "echo fg" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("bash");
    let fg_text = text_of(&fg);
    assert!(
        fg_text.contains("fg") && fg_text.contains("[exit 0]"),
        "got: {fg_text}"
    );
}

#[tokio::test]
async fn execute_missing_command_errors() {
    let tool = BashTool;

    let err = tool
        .execute("m1", json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing command must fail");

    assert_eq!(err.to_string(), "missing `command`");
}

#[tokio::test]
async fn execute_honors_cwd_param() {
    // Arrange: run `pwd` inside a temp dir so the cwd param is observable.
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().to_string_lossy().into_owned();
    let tool = BashTool;

    // Act
    let result = tool
        .execute(
            "m2",
            json!({ "command": "pwd", "cwd": cwd }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("pwd in tempdir should succeed");

    // Assert: the tool ran the command with the requested cwd.
    let text = text_of(&result);
    assert!(
        text.contains(&format!("$ pwd\n{cwd}\n[exit 0]")),
        "got: {text}"
    );
}

#[tokio::test]
async fn execute_adds_newline_after_stdout_without_trailing_newline() {
    let tool = BashTool;

    let result = tool
        .execute("m3", json!({ "command": "printf hello" }), CancellationToken::new(), None)
        .await
        .expect("printf should succeed");

    let text = text_of(&result);
    assert!(
        text.contains("hello\n[exit 0]"),
        "stdout without trailing newline must get one before the exit marker: {text}"
    );
}

#[tokio::test]
async fn execute_adds_newline_after_stderr_without_trailing_newline() {
    let tool = BashTool;

    let result = tool
        .execute("m4", json!({ "command": "printf err >&2" }), CancellationToken::new(), None)
        .await
        .expect("printf to stderr should succeed");

    let text = text_of(&result);
    assert!(
        text.contains("[stderr]\nerr\n[exit 0]"),
        "stderr without trailing newline must get one before the exit marker: {text}"
    );
}
