//! Tests for `write` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use theway_core::executor::{CommandOutput, ExecutorError, ExecutorKind, ToolExecutor};

type ExecutorResult<T> = Result<T, ExecutorError>;

#[cfg(feature = "local")]
fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

/// In-memory executor double for `write`: succeeds or fails on `write_file`
/// and records the writes. All other operations fail — they must not be used
/// by the write tool.
struct FakeWriteExecutor {
    fail_with: Option<String>,
    written: std::sync::Mutex<Vec<(std::path::PathBuf, String)>>,
}

impl FakeWriteExecutor {
    fn ok() -> Self {
        Self {
            fail_with: None,
            written: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn failing(msg: &str) -> Self {
        Self {
            fail_with: Some(msg.to_string()),
            written: std::sync::Mutex::new(Vec::new()),
        }
    }

}

#[async_trait]
impl ToolExecutor for FakeWriteExecutor {
    async fn kind(&self) -> ExecutorKind {
        ExecutorKind::Local
    }

    async fn read_file(&self, _path: &Path) -> ExecutorResult<String> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn write_file(&self, path: &Path, content: &str) -> ExecutorResult<()> {
        self.written
            .lock()
            .unwrap()
            .push((path.to_path_buf(), content.to_string()));
        match &self.fail_with {
            Some(msg) => Err(ExecutorError::Other(msg.clone())),
            None => Ok(()),
        }
    }

    async fn run_command(
        &self,
        _cwd: &Path,
        _argv: &[String],
        _timeout: Duration,
    ) -> ExecutorResult<CommandOutput> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
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

#[test]
fn definition_exposes_write_schema_and_label() {
    let tool = WriteTool::new(std::sync::Arc::new(FakeWriteExecutor::ok()));

    assert_eq!(tool.label(), "write");
    assert_eq!(tool.definition().name, "write");
    assert_eq!(tool.definition().parameters["required"][0], "path");
    assert_eq!(tool.definition().parameters["required"][1], "content");
}

#[tokio::test]
async fn execute_missing_path_errors() {
    // Arrange
    let tool = WriteTool::new(std::sync::Arc::new(FakeWriteExecutor::ok()));

    // Act
    let err = tool
        .execute(
            "id-1",
            json!({ "content": "hello" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("missing path must fail");

    // Assert
    assert_eq!(err.to_string(), "missing `path`");
}

#[tokio::test]
async fn execute_missing_content_errors() {
    // Arrange
    let tool = WriteTool::new(std::sync::Arc::new(FakeWriteExecutor::ok()));

    // Act
    let err = tool
        .execute("id-2", json!({ "path": "a.txt" }), CancellationToken::new(), None)
        .await
        .expect_err("missing content must fail");

    // Assert
    assert_eq!(err.to_string(), "missing `content`");
}

#[tokio::test]
async fn execute_maps_executor_error_with_path_context() {
    // Arrange
    let tool = WriteTool::new(std::sync::Arc::new(FakeWriteExecutor::failing("disk on fire")));

    // Act
    let err = tool
        .execute(
            "id-3",
            json!({ "path": "a.txt", "content": "hi" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("executor failure must propagate");

    // Assert
    let msg = err.to_string();
    assert!(msg.contains("write a.txt: executor error: disk on fire"), "got: {msg}");
}

#[cfg(feature = "local")]
#[tokio::test]
async fn execute_writes_file_and_reports_bytes_and_lines() {
    // Arrange: temp dir with a nested target path (parent creation is part of
    // the write contract).
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("sub").join("note.txt");
    let content = "hello\nworld\n";
    let tool = WriteTool::new(std::sync::Arc::new(
        theway_daemon::executor::local::LocalExecutor::new(),
    ));

    // Act
    let result = tool
        .execute(
            "id-4",
            json!({ "path": file.to_string_lossy(), "content": content }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("write should succeed");

    // Assert
    let text = text_of(&result);
    assert!(
        text.contains(&format!(
            "Wrote 12 bytes (2 lines) to {}",
            file.to_string_lossy()
        )),
        "got: {text}"
    );
    assert_eq!(std::fs::read_to_string(&file).expect("read back"), content);
    assert_eq!(result.details["path"], json!(file.to_string_lossy()));
    assert_eq!(result.details["bytes"], 12);
    assert_eq!(result.details["lines"], 2);
}

#[cfg(feature = "local")]
#[tokio::test]
async fn execute_empty_content_reports_zero_counts() {
    // Arrange
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("empty.txt");
    let tool = WriteTool::new(std::sync::Arc::new(
        theway_daemon::executor::local::LocalExecutor::new(),
    ));

    // Act
    let result = tool
        .execute(
            "id-5",
            json!({ "path": file.to_string_lossy(), "content": "" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("empty write should succeed");

    // Assert
    let text = text_of(&result);
    assert!(text.contains("Wrote 0 bytes (0 lines)"), "got: {text}");
    assert_eq!(std::fs::read_to_string(&file).expect("read back"), "");
}
