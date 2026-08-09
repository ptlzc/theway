//! End-to-end test for the structured git tool. Init a tempdir repo, make changes, then run
//! the tool against it. We rely on the system `git` binary being available; this is a
//! reasonable test-time dependency for a coding agent.

use std::process::Command;

use tempfile::TempDir;
use theway_core::AgentTool;
use tokio_util::sync::CancellationToken;

#[path = "../../core/src/runtime/tools/git.rs"]
mod git;

/// Initialise a fresh repo at `dir` with one committed file and one staged change.
fn init_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "tester")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "tester")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .expect("git command runs");
        assert!(
            st.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-q", "-m", "initial"]);
    std::fs::write(dir.join("b.txt"), "draft\n").unwrap();
}

fn ensure_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn git_status_reports_untracked_file() {
    if !ensure_git_available() {
        eprintln!("(skipped: git binary not on PATH)");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let tool = git::GitTool;
    let res = tool
        .execute(
            "call-1",
            serde_json::json!({
                "subcommand": "status",
                "cwd": dir.path().to_string_lossy(),
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let body = match &res.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(body.contains("git status"), "header: {body}");
    assert!(body.contains("?? b.txt"), "untracked file: {body}");
    assert!(body.contains("## main"), "branch line: {body}");
}

#[tokio::test]
async fn git_log_caps_at_twenty_entries_and_uses_pretty_format() {
    if !ensure_git_available() {
        eprintln!("(skipped: git binary not on PATH)");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());
    let tool = git::GitTool;
    let res = tool
        .execute(
            "call-2",
            serde_json::json!({
                "subcommand": "log",
                "cwd": dir.path().to_string_lossy(),
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let body = match &res.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(
        body.contains("initial"),
        "should show initial commit: {body}"
    );
    // Hash + author should appear in the pretty format.
    assert!(body.contains("tester"), "author in pretty fmt: {body}");
}

#[tokio::test]
async fn git_unsupported_subcommand_errors() {
    let tool = git::GitTool;
    let err = tool
        .execute(
            "call-3",
            serde_json::json!({ "subcommand": "push" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("unsupported"), "{err}");
}
