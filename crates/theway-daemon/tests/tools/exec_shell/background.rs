use super::*;

#[tokio::test]
async fn background_shell_get_output_waits_and_reports_exit() {
    let _registry = registry_test_lock();
    let bg = run_in_background(&format!("{} && echo hello", short_sleep_cmd()))
        .await
        .expect("spawn");
    let tool = GetOutputTool;
    let result = tool
        .execute(
            "g1",
            json!({ "shell_id": bg.id, "timeout": 15 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("get_output");
    let text = text_of(&result);
    assert!(text.contains("hello"), "expected output, got: {text}");
    assert!(text.contains(&format!("[{}]", bg.id)), "got: {text}");

    // The command exits on its own; a follow-up read reports the exit code.
    let result2 = tool
        .execute(
            "g2",
            json!({ "shell_id": bg.id, "timeout": 15 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("get_output");
    assert!(
        text_of(&result2).contains("exited (code 0)"),
        "expected exited, got: {}",
        text_of(&result2)
    );
}

#[tokio::test]
async fn alive_count_tracks_running_shells() {
    let _registry = registry_test_lock();
    let before = registry().alive_count();

    let bg = run_in_background(long_sleep_cmd()).await.expect("spawn");
    // A live (not yet exited) shell counts toward alive_count.
    assert_eq!(
        registry().alive_count(),
        before + 1,
        "running shell should be counted as alive"
    );

    // Clean up so we don't leak a background process across tests.
    KillShellTool
        .execute(
            "alive1",
            json!({ "shell_id": bg.id }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("cleanup kill");
    // kill_shell removes the entry from the registry, so it no longer counts.
    assert!(
        registry().get(&bg.id).is_none(),
        "shell should be removed after kill"
    );
    assert_eq!(
        registry().alive_count(),
        before,
        "killed/removed shell should not be counted as alive"
    );
}

#[tokio::test]
async fn alive_count_excludes_exited_shells() {
    let _registry = registry_test_lock();
    let before = registry().alive_count();

    // A short-lived command exits on its own; once marked exited (retained handle
    // still queryable), it must not count as alive.
    let bg = run_in_background(short_sleep_cmd()).await.expect("spawn");
    let handle = registry().get(&bg.id).expect("registered");

    // Wait for natural exit so the handle is marked exited.
    get_output_text(&handle, Some(15), &CancellationToken::new()).await;
    assert!(
        handle.exited.load(std::sync::atomic::Ordering::SeqCst),
        "shell should have exited"
    );
    assert_eq!(
        registry().alive_count(),
        before,
        "exited shell should not be counted as alive"
    );
}

#[tokio::test]
async fn kill_shell_terminates_background_process() {
    let _registry = registry_test_lock();
    let bg = run_in_background(long_sleep_cmd()).await.expect("spawn");
    let handle = registry().get(&bg.id).expect("registered");

    let result = KillShellTool
        .execute(
            "k1",
            json!({ "shell_id": bg.id }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("kill_shell");
    assert!(
        text_of(&result).contains("Killed"),
        "got: {}",
        text_of(&result)
    );

    // Removed from the registry: a tool-level get_output now reports unknown.
    assert!(registry().get(&bg.id).is_none(), "shell still registered");
    assert!(
        GetOutputTool
            .execute(
                "k2",
                json!({ "shell_id": bg.id }),
                CancellationToken::new(),
                None,
            )
            .await
            .is_err(),
        "get_output on a killed shell should error"
    );

    // The retained handle observes the exit once the watcher reaps the killed tree.
    let text = get_output_text(&handle, Some(10), &CancellationToken::new()).await;
    assert!(
        text.contains("exited"),
        "expected exited after kill, got: {text}"
    );
}

/// `kill_shell` must kill not just the direct shell but any descendants it
/// backgrounded — same leak shape the bash / native-env regression tests cover for
/// timeout/cancel: `(sleep N; touch marker) & wait` backgrounds the subshell, so a
/// direct-child-only kill would leave the descendant alive to write the marker.
/// `run_in_background` spawns through the shared process-group primitive (setsid at
/// spawn) and `kill_shell` kills the whole group by pid. Unix-only because
/// `setsid` / `killpg` are Unix primitives.
#[cfg(unix)]
#[tokio::test]
async fn kill_shell_kills_backgrounded_descendant_processes() {
    let _registry = registry_test_lock();
    use tempfile::tempdir;
    let dir = tempdir().expect("tempdir");
    let marker = dir.path().join("exec-shell-leak-marker");
    let marker_str = marker.to_string_lossy().to_string();

    let bg = run_in_background(&format!("(sleep 4; touch {marker_str}) & wait"))
        .await
        .expect("spawn");

    // Give the backgrounded subshell a beat to actually fork before we kill.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let result = KillShellTool
        .execute(
            "kd1",
            json!({ "shell_id": bg.id }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("kill_shell");
    assert!(
        text_of(&result).contains("Killed"),
        "got: {}",
        text_of(&result)
    );

    // Wider window than the descendant's 4s sleep: if the killpg missed the
    // backgrounded subshell, the marker file appears unambiguously.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !marker.exists(),
        "descendant process was not killed by kill_shell — leak marker at {marker_str} exists"
    );
}

#[tokio::test]
async fn write_to_process_writes_stdin() {
    let _registry = registry_test_lock();
    let bg = run_in_background(stdin_echo_cmd()).await.expect("spawn");
    let handle = registry().get(&bg.id).expect("registered");

    let result = WriteToProcessTool
        .execute(
            "w1",
            json!({ "shell_id": bg.id, "text_input": "hello\n" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("write_to_process");
    assert!(
        text_of(&result).contains("Wrote 6 bytes"),
        "got: {}",
        text_of(&result)
    );

    // cat / more echo stdin back on stdout.
    let out = get_output_text(&handle, Some(10), &CancellationToken::new()).await;
    assert!(out.contains("hello"), "expected echoed input, got: {out}");

    // cat / more never exit on their own — clean up.
    KillShellTool
        .execute(
            "w2",
            json!({ "shell_id": bg.id }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("cleanup kill");
}
