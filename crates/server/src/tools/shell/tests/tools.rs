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
async fn bash_run_in_background_returns_shell_id() {
    // Relative on purpose: this module also compiles inside integration tests that pull
    // `tools/` in via `#[path]` at a different crate-root depth (server tests/tools.rs).
    let tool = super::super::super::bash::BashTool;
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
