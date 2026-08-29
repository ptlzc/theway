//! Tests for `git` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use theway_core::executor::{CommandOutput, ExecutorError, ExecutorKind, ToolExecutor};

type ExecutorResult<T> = Result<T, ExecutorError>;

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedRun {
    cwd: PathBuf,
    argv: Vec<String>,
    timeout_ms: u64,
}

/// In-memory executor double for `git`: records `run_command` calls and
/// returns canned output. `hang` keeps `run_command` pending so cancellation
/// can be exercised deterministically. All other operations fail — the git
/// tool only uses `run_command`.
struct FakeGitExecutor {
    stdout: String,
    stderr: String,
    exit_code: i32,
    fail_with: Option<String>,
    hang: bool,
    calls: Mutex<Vec<RecordedRun>>,
}

impl FakeGitExecutor {
    fn success(stdout: &str, stderr: &str, exit_code: i32) -> Self {
        Self {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code,
            fail_with: None,
            hang: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn failing(msg: &str) -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            fail_with: Some(msg.to_string()),
            hang: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn hanging() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            fail_with: None,
            hang: true,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn recorded(&self) -> Vec<RecordedRun> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolExecutor for FakeGitExecutor {
    async fn kind(&self) -> ExecutorKind {
        ExecutorKind::Local
    }

    async fn read_file(&self, _path: &Path) -> ExecutorResult<String> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn write_file(&self, _path: &Path, _content: &str) -> ExecutorResult<()> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn run_command(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Duration,
    ) -> ExecutorResult<CommandOutput> {
        if self.hang {
            std::future::pending::<()>().await;
            unreachable!("hanging executor should never resolve");
        }
        self.calls.lock().unwrap().push(RecordedRun {
            cwd: cwd.to_path_buf(),
            argv: argv.to_vec(),
            timeout_ms: timeout.as_millis() as u64,
        });
        if let Some(msg) = &self.fail_with {
            return Err(ExecutorError::Other(msg.clone()));
        }
        Ok(CommandOutput {
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            exit_code: self.exit_code,
        })
    }

    async fn list_dir(&self, _path: &Path) -> ExecutorResult<Vec<String>> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn grep(&self, _pattern: &str, _path: &Path) -> ExecutorResult<Vec<String>> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn find(&self, _glob: &str, _path: &Path) -> ExecutorResult<Vec<String>> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn git(&self, _args: &[String]) -> ExecutorResult<CommandOutput> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }
}

fn tool_with(executor: FakeGitExecutor) -> GitTool {
    GitTool::new(Arc::new(executor))
}

async fn exec(tool: &GitTool, params: Value) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("id", params, CancellationToken::new(), None)
        .await
}

#[test]
fn definition_exposes_git_schema_and_label() {
    let tool = tool_with(FakeGitExecutor::success("", "", 0));

    assert_eq!(tool.label(), "git");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert_eq!(tool.definition().name, "git");
    assert_eq!(tool.definition().parameters["required"][0], "subcommand");
    assert_eq!(
        tool.definition().parameters["properties"]["subcommand"]["enum"],
        json!(["status", "diff", "log"])
    );
}

#[test]
fn build_argv_adds_defaults_per_subcommand_and_appends_extra_args() {
    // Arrange
    let extra = ["--ignored".to_string()];

    // Act & Assert
    assert_eq!(build_argv("status", &[]), vec!["status", "--short", "--branch"]);
    assert_eq!(
        build_argv("diff", &[]),
        vec!["diff", "--no-color", "--no-ext-diff"]
    );
    assert_eq!(
        build_argv("log", &[]),
        vec!["log", "--no-color", "-n", "20", "--pretty=format:%h %ci %an %s"]
    );
    assert_eq!(build_argv("push", &extra), vec!["push", "--ignored"]);
    assert_eq!(
        build_argv("status", &extra),
        vec!["status", "--short", "--branch", "--ignored"]
    );
}

#[test]
fn truncate_under_limit_returns_unchanged_and_not_truncated() {
    // Arrange
    let s = "small output".to_string();

    // Act
    let (out, truncated) = truncate(&s);

    // Assert
    assert_eq!(out, s);
    assert!(!truncated);
}

#[test]
fn truncate_over_limit_steps_back_to_char_boundary() {
    // Arrange: MAX_OUTPUT_BYTES-1 ASCII bytes, then a 2-byte `é` straddling MAX_OUTPUT_BYTES.
    let prefix = "a".repeat(MAX_OUTPUT_BYTES - 1);
    let s = format!("{prefix}é{}", "b".repeat(100));
    assert!(s.len() > MAX_OUTPUT_BYTES);

    // Act
    let (out, truncated) = truncate(&s);

    // Assert: the cut lands on the boundary before `é`, never mid-char.
    assert!(truncated);
    assert_eq!(out, prefix);
}

#[tokio::test]
async fn execute_missing_subcommand_errors() {
    // Arrange
    let tool = tool_with(FakeGitExecutor::success("", "", 0));

    // Act
    let err = exec(&tool, json!({})).await.expect_err("missing subcommand");

    // Assert
    assert_eq!(err.to_string(), "missing required arg: subcommand");
}

#[tokio::test]
async fn execute_rejects_unsupported_subcommand() {
    // Arrange
    let tool = tool_with(FakeGitExecutor::success("", "", 0));

    // Act
    let err = exec(&tool, json!({ "subcommand": "push" }))
        .await
        .expect_err("push must be unsupported");

    // Assert
    assert!(
        err.to_string()
            .contains("unsupported git subcommand: push (allowed: status, diff, log)"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_renders_success_with_header_and_details() {
    // Arrange
    let executor = Arc::new(FakeGitExecutor::success(" M file.txt\n", "", 0));
    let tool = GitTool::new(executor.clone());

    // Act
    let result = exec(
        &tool,
        json!({ "subcommand": "status", "cwd": "/tmp/repo", "args": ["--ignored"] }),
    )
    .await
    .expect("git status should succeed");

    // Assert
    let text = text_of(&result);
    assert!(
        text.starts_with("git status (cwd=/tmp/repo)\n"),
        "header missing: {text}"
    );
    assert!(text.ends_with(" M file.txt\n"), "body missing: {text}");
    assert!(!text.contains("truncated"), "should not truncate: {text}");
    assert_eq!(result.details["subcommand"], "status");
    assert_eq!(result.details["exit_status"], 0);
    assert_eq!(result.details["truncated"], false);
    assert_eq!(
        result.details["full_text"],
        "git status (cwd=/tmp/repo)\n M file.txt\n"
    );
    assert_eq!(
        result.details["argv"],
        json!(["status", "--short", "--branch", "--ignored"])
    );

    let calls = executor.recorded();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].cwd, PathBuf::from("/tmp/repo"));
    assert_eq!(
        calls[0].argv,
        vec!["git", "status", "--short", "--branch", "--ignored"]
    );
    assert_eq!(calls[0].timeout_ms, 60_000);
}

#[tokio::test]
async fn execute_renders_stderr_when_git_exits_nonzero() {
    // Arrange
    let executor = Arc::new(FakeGitExecutor::success("", "fatal: bad revision\n", 128));
    let tool = GitTool::new(executor.clone());

    // Act
    let result = exec(&tool, json!({ "subcommand": "log" }))
        .await
        .expect("git log failure is reported as structured output, not Err");

    // Assert: cwd defaults to ".", nonzero output includes stderr section.
    let text = text_of(&result);
    assert!(
        text.starts_with("git log (cwd=.)\n"),
        "header missing: {text}"
    );
    assert!(
        text.contains("git log exited with status 128\n--- stderr ---\nfatal: bad revision"),
        "stderr body missing: {text}"
    );
    assert_eq!(result.details["exit_status"], 128);
    assert_eq!(executor.recorded()[0].cwd, PathBuf::from("."));
}

#[tokio::test]
async fn execute_maps_executor_error() {
    // Arrange
    let tool = tool_with(FakeGitExecutor::failing("spawn git: boom"));

    // Act
    let err = exec(&tool, json!({ "subcommand": "status" }))
        .await
        .expect_err("executor failure must propagate");

    // Assert
    assert!(err.to_string().contains("git: executor error: spawn git: boom"), "got: {err}");
}

#[tokio::test]
async fn execute_honors_cancellation_before_run_command_completes() {
    // Arrange: a run_command future that never resolves, and a token that is
    // already cancelled.
    let tool = tool_with(FakeGitExecutor::hanging());
    let cancel = CancellationToken::new();
    cancel.cancel();

    // Act
    let err = tool
        .execute("id", json!({ "subcommand": "status" }), cancel, None)
        .await
        .expect_err("cancellation must win over a pending run_command");

    // Assert
    assert_eq!(err.to_string(), "cancelled");
}

#[cfg(feature = "local")]
#[tokio::test]
async fn execute_runs_git_status_in_local_repo() {
    // Arrange: a local temp repo with one untracked file. No remotes, no network.
    if !git_available() {
        eprintln!("(skipped: git binary not on PATH)");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let init = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir.path())
        .status()
        .expect("run git init");
    if !init.success() {
        eprintln!("(skipped: git init failed)");
        return;
    }
    std::fs::write(dir.path().join("a.txt"), "hello\n").expect("write fixture");
    let tool = GitTool::new(Arc::new(theway_daemon::executor::local::LocalExecutor::new()));

    // Act
    let result = tool
        .execute(
            "id",
            json!({ "subcommand": "status", "cwd": dir.path().to_string_lossy() }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("git status should succeed in a local repo");

    // Assert
    let text = text_of(&result);
    assert!(
        text.starts_with("git status (cwd="),
        "header missing: {text}"
    );
    assert!(text.contains("?? a.txt"), "untracked file missing: {text}");
    assert_eq!(result.details["exit_status"], 0);
}

#[cfg(feature = "local")]
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
