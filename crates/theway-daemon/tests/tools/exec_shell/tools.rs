use super::*;

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
