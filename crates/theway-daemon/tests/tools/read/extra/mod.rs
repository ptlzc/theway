//! Additional tests for `read` — kept in a separate bridged module so the
//! original inline suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use theway_core::ToolExecutionMode;
use theway_core::executor::{CommandOutput, ExecutorError, ExecutorKind, ToolExecutor};

fn local_executor() -> Arc<dyn ToolExecutor> {
    Arc::new(crate::executor::local::LocalExecutor::new())
}

struct FailingReadExecutor;

#[async_trait::async_trait]
impl ToolExecutor for FailingReadExecutor {
    async fn kind(&self) -> ExecutorKind {
        ExecutorKind::Local
    }

    async fn read_file(&self, _path: &Path) -> Result<String, ExecutorError> {
        Err(ExecutorError::Other("boom".into()))
    }

    async fn write_file(&self, _path: &Path, _content: &str) -> Result<(), ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn run_command(
        &self,
        _cwd: &Path,
        _argv: &[String],
        _timeout: Duration,
    ) -> Result<CommandOutput, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn list_dir(&self, _path: &Path) -> Result<Vec<String>, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn grep(&self, _pattern: &str, _path: &Path) -> Result<Vec<String>, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn find(&self, _glob: &str, _path: &Path) -> Result<Vec<String>, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn git(&self, _args: &[String]) -> Result<CommandOutput, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }
}

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

#[test]
fn definition_label_and_execution_mode_are_registered() {
    let tool = ReadTool::new(local_executor());

    assert_eq!(tool.definition().name, "read");
    assert_eq!(tool.label(), "read");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert_eq!(tool.definition().parameters["required"][0], "path");
}

#[tokio::test]
async fn execute_missing_path_is_error() {
    let tool = ReadTool::new(local_executor());

    let err = tool
        .execute("r", serde_json::json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing path must fail");

    assert_eq!(err.to_string(), "missing `path`");
}

#[tokio::test]
async fn execute_non_numeric_offset_treats_as_one_and_suppresses_hint() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.txt");
    let mut body = String::new();
    for i in 0..250 {
        body.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&p, body).unwrap();
    let tool = ReadTool::new(local_executor());

    let result = tool
        .execute(
            "r",
            serde_json::json!({ "path": p.to_str().unwrap(), "offset": "abc" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("read with non-numeric offset must not fail");

    let text = text_of(&result);
    assert!(!text.contains("use outline for structure"), "got: {text}");
    assert!(
        text.starts_with(&format!("[{}] lines 1-", p.to_str().unwrap())),
        "got: {text}"
    );
    assert!(text.contains("line 0"), "got: {text}");
}

#[tokio::test]
async fn execute_limit_limits_returned_lines() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("small.txt");
    let body = "line0\nline1\nline2\nline3\nline4\n";
    std::fs::write(&p, body).unwrap();
    let tool = ReadTool::new(local_executor());

    let result = tool
        .execute(
            "r",
            serde_json::json!({ "path": p.to_str().unwrap(), "limit": 3 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("read with limit must succeed");

    let text = text_of(&result);
    assert!(text.contains("line0"), "got: {text}");
    assert!(text.contains("line2"), "got: {text}");
    assert!(!text.contains("line3"), "got: {text}");
    assert_eq!(result.details["keptLines"], 3);
}

#[tokio::test]
async fn execute_large_lines_are_not_byte_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("wide.txt");
    let mut body = String::new();
    body.push_str(&"a".repeat(200 * 1024));
    body.push('\n');
    body.push_str(&"b".repeat(200 * 1024));
    body.push('\n');
    std::fs::write(&p, body).unwrap();
    let tool = ReadTool::new(local_executor());

    let result = tool
        .execute(
            "r",
            serde_json::json!({ "path": p.to_str().unwrap() }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("large-line read must still succeed");

    let text = text_of(&result);
    assert!(!text.contains("[truncated"), "got: {text}");
    assert!(
        text.contains(&"a".repeat(200 * 1024)),
        "first large line must be returned in full"
    );
    assert!(
        text.contains(&"b".repeat(200 * 1024)),
        "second large line must be returned in full"
    );
}

#[tokio::test]
async fn execute_read_error_maps_with_path_context() {
    let tool = ReadTool::new(Arc::new(FailingReadExecutor));

    let err = tool
        .execute(
            "r",
            serde_json::json!({ "path": "/tmp/nope.txt" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("executor failure must surface");

    let msg = err.to_string();
    assert!(
        msg.contains("read /tmp/nope.txt:") && msg.contains("boom"),
        "got: {msg}"
    );
}
