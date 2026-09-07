use super::*;
use std::time::Duration;

#[tokio::test]
async fn exec_foreground_matches_bash() {
    let result = ExecTool
        .execute(
            "e1",
            json!({ "command": "echo hello" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("exec");
    let text = text_of(&result);
    assert!(
        text.contains("hello") && text.contains("[exit 0]"),
        "got: {text}"
    );
}

#[tokio::test]
async fn exec_background_returns_shell_id() {
    let _registry = registry_test_lock();
    let result = ExecTool
        .execute(
            "e2",
            json!({ "command": "echo bg", "run_in_background": true }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("exec");
    let text = text_of(&result);
    assert!(
        text.contains("background shell started: shell-"),
        "got: {text}"
    );

    // Recover the id from the message and read the output back.
    let id = format!(
        "shell-{}",
        text.split("shell-")
            .nth(1)
            .expect("id")
            .split_whitespace()
            .next()
            .expect("num")
    );
    let handle = registry().get(&id).expect("registered");
    let out = get_output_text(&handle, Some(10), &CancellationToken::new()).await;
    assert!(out.contains("bg"), "got: {out}");
}

#[tokio::test]
async fn exec_foreground_timeout_kills_process_tree() {
    let outcome = crate::tools::exec::run_with_kill_on_timeout_or_cancel(
        "sleep 60",
        Some(Duration::from_secs(1)),
        None,
        None,
        &CancellationToken::new(),
    )
    .await
    .expect("timeout path folds into the outcome, not an error");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(
        outcome.kill_reason,
        Some(crate::tools::exec::KillReason::TimedOut { secs: 1 })
    );
    let rendered = outcome.rendered_exit();
    assert_eq!(rendered, -1);
    assert!(
        outcome
            .stderr_suffix
            .as_deref()
            .unwrap_or("")
            .contains("timed out after 1s"),
        "got: {:?}",
        outcome.stderr_suffix
    );
}

#[tokio::test]
async fn exec_foreground_cancel_kills_process_tree() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let outcome = crate::tools::exec::run_with_kill_on_timeout_or_cancel(
        "sleep 60",
        None,
        None,
        None,
        &cancel,
    )
    .await
    .expect("cancel path folds into the outcome, not an error");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(
        outcome.kill_reason,
        Some(crate::tools::exec::KillReason::Cancelled)
    );
    assert_eq!(outcome.rendered_exit(), -1);
    assert_eq!(outcome.stderr_suffix.as_deref(), Some("[aborted]"));
}

#[tokio::test]
async fn exec_missing_command_is_rejected() {
    let err = ExecTool
        .execute("e3", json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing command must fail");
    assert!(err.to_string().contains("missing `command`"), "got: {err}");
}

#[tokio::test]
async fn exec_foreground_honors_explicit_timeout() {
    let result = ExecTool
        .execute(
            "e4",
            json!({ "command": "echo fast", "timeout": 5 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("explicit timeout should not change normal execution");
    let text = text_of(&result);
    assert!(text.contains("fast") && text.contains("[exit 0]"), "got: {text}");
}

#[tokio::test]
async fn exec_background_honors_cwd() {
    let _registry = registry_test_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let result = ExecTool
        .execute(
            "e5",
            json!({
                "command": "pwd",
                "run_in_background": true,
                "cwd": dir.path().to_str().unwrap(),
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("background exec with cwd");
    let text = text_of(&result);
    let id = format!(
        "shell-{}",
        text.split("shell-").nth(1).unwrap().split_whitespace().next().unwrap()
    );
    let handle = registry().get(&id).expect("registered");
    let out = get_output_text(&handle, Some(10), &CancellationToken::new()).await;
    assert!(
        out.contains(&dir.path().canonicalize().unwrap().to_string_lossy().to_string()),
        "got: {out}"
    );
}

#[tokio::test]
async fn get_output_missing_shell_id_is_rejected() {
    let err = GetOutputTool
        .execute("g3", json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing shell_id must fail");
    assert!(err.to_string().contains("missing `shell_id`"), "got: {err}");
}

#[tokio::test]
async fn get_output_unknown_shell_lists_available() {
    let _registry = registry_test_lock();
    let bg = run_in_background(long_sleep_cmd()).await.expect("spawn");
    let err = GetOutputTool
        .execute(
            "g4",
            json!({ "shell_id": "shell-99999" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("unknown shell_id must fail");
    let msg = err.to_string();
    assert!(msg.contains("Unknown shell_id"), "got: {msg}");
    assert!(
        msg.contains(&bg.id),
        "available list should name {}: {msg}",
        bg.id
    );
    KillShellTool
        .execute("cleanup-g4", json!({ "shell_id": bg.id }), CancellationToken::new(), None)
        .await
        .expect("cleanup kill");
}

#[tokio::test]
async fn get_output_timeout_returns_running_snapshot() {
    let _registry = registry_test_lock();
    let bg = run_in_background(long_sleep_cmd()).await.expect("spawn");
    let handle = registry().get(&bg.id).expect("registered");
    let text = get_output_text(&handle, Some(1), &CancellationToken::new()).await;
    assert!(text.contains("running"), "got: {text}");
    assert!(text.contains("stdout:"), "got: {text}");
    KillShellTool
        .execute("cleanup-timeout", json!({ "shell_id": bg.id }), CancellationToken::new(), None)
        .await
        .expect("cleanup kill");
}

#[tokio::test]
async fn get_output_cancelled_returns_running_snapshot() {
    let _registry = registry_test_lock();
    let bg = run_in_background(long_sleep_cmd()).await.expect("spawn");
    let handle = registry().get(&bg.id).expect("registered");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let text = get_output_text(&handle, None, &cancel).await;
    assert!(text.contains("running"), "got: {text}");
    KillShellTool
        .execute("cleanup-cancel", json!({ "shell_id": bg.id }), CancellationToken::new(), None)
        .await
        .expect("cleanup kill");
}

#[tokio::test]
async fn kill_shell_missing_shell_id_is_rejected() {
    let err = KillShellTool
        .execute("k3", json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing shell_id must fail");
    assert!(err.to_string().contains("missing `shell_id`"), "got: {err}");
}

#[tokio::test]
async fn kill_shell_unknown_shell_id_is_rejected() {
    let _registry = registry_test_lock();
    let err = KillShellTool
        .execute("k4", json!({ "shell_id": "shell-99999" }), CancellationToken::new(), None)
        .await
        .expect_err("unknown shell_id must fail");
    assert!(err.to_string().contains("Unknown shell_id"), "got: {err}");
}

#[tokio::test]
async fn write_to_process_missing_shell_id_is_rejected() {
    let err = WriteToProcessTool
        .execute("w5", json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing shell_id must fail");
    assert!(err.to_string().contains("missing `shell_id`"), "got: {err}");
}

#[tokio::test]
async fn write_to_process_unknown_shell_id_is_rejected() {
    let _registry = registry_test_lock();
    let err = WriteToProcessTool
        .execute("w6", json!({ "shell_id": "shell-99999" }), CancellationToken::new(), None)
        .await
        .expect_err("unknown shell_id must fail");
    assert!(err.to_string().contains("Unknown shell_id"), "got: {err}");
}

#[tokio::test]
async fn write_to_process_empty_text_is_accepted() {
    let _registry = registry_test_lock();
    let bg = run_in_background(stdin_echo_cmd()).await.expect("spawn");
    let result = WriteToProcessTool
        .execute(
            "w7",
            json!({ "shell_id": bg.id }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("empty text_input defaults to empty write");
    assert!(text_of(&result).contains("Wrote 0 bytes"), "got: {}", text_of(&result));
    KillShellTool
        .execute("cleanup-w7", json!({ "shell_id": bg.id }), CancellationToken::new(), None)
        .await
        .expect("cleanup kill");
}

#[tokio::test]
async fn exec_foreground_honors_cwd_and_envs() {
    use std::collections::BTreeMap;


    let dir = tempfile::tempdir().expect("tempdir");
    let mut envs = BTreeMap::new();
    envs.insert("THEWAY_TEST_ENV".to_string(), "hello-from-env".to_string());
    let outcome = crate::tools::exec::run_with_kill_on_timeout_or_cancel(
        "printf '%s' \"$THEWAY_TEST_ENV\"",
        Some(Duration::from_secs(5)),
        Some(dir.path()),
        Some(&envs),
        &CancellationToken::new(),
    )
    .await
    .expect("foreground command should complete");
    assert_eq!(outcome.stdout, "hello-from-env");
    assert_eq!(outcome.exit_code, Some(0));
}

#[tokio::test]
async fn terminate_child_tree_without_pid_reaps_child() {
    use std::process::Stdio;

    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn child");
    crate::tools::exec::process_group::terminate_child_tree(&mut child, None).await;
}
