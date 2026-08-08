use super::*;

#[tokio::test]
async fn background_shell_get_output_waits_and_reports_exit() {
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
async fn kill_shell_terminates_background_process() {
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

#[tokio::test]
async fn write_to_process_writes_stdin() {
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
