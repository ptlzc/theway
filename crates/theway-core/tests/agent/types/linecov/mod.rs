//! Additional line-coverage tests for `agent::types` (see docs/rust-test-files.md).

use super::super::*;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

struct NoopEnv;

#[async_trait]
impl ExecutionEnv for NoopEnv {
    fn cwd(&self) -> &str {
        "/"
    }

    async fn absolute_path(&self, path: &str, _cancel: CancellationToken) -> FsResult<String> {
        Ok(path.to_string())
    }

    async fn join_path(&self, parts: &[&str], _cancel: CancellationToken) -> FsResult<String> {
        Ok(parts.join("/"))
    }

    async fn read_text_file(&self, _path: &str, _cancel: CancellationToken) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::NotFound, "no content"))
    }

    async fn read_text_lines(
        &self,
        _path: &str,
        _max_lines: Option<usize>,
        _cancel: CancellationToken,
    ) -> FsResult<Vec<String>> {
        Err(FileError::new(FileErrorCode::NotFound, "no content"))
    }

    async fn read_binary_file(&self, _path: &str, _cancel: CancellationToken) -> FsResult<Vec<u8>> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &[u8],
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn append_file(
        &self,
        _path: &str,
        _content: &[u8],
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn file_info(&self, path: &str, _cancel: CancellationToken) -> FsResult<FileInfo> {
        Err(FileError::new(FileErrorCode::NotFound, "missing").with_path(path))
    }

    async fn list_dir(&self, path: &str, _cancel: CancellationToken) -> FsResult<Vec<FileInfo>> {
        Err(FileError::new(FileErrorCode::NotFound, "missing").with_path(path))
    }

    async fn exists(&self, _path: &str, _cancel: CancellationToken) -> FsResult<bool> {
        Ok(false)
    }

    async fn canonical_path(&self, path: &str, _cancel: CancellationToken) -> FsResult<String> {
        Ok(path.to_string())
    }

    async fn create_dir(
        &self,
        _path: &str,
        _recursive: bool,
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn remove(
        &self,
        _path: &str,
        _recursive: bool,
        _force: bool,
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn create_temp_dir(
        &self,
        _prefix: Option<&str>,
        _cancel: CancellationToken,
    ) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn create_temp_file(
        &self,
        _prefix: Option<&str>,
        _suffix: Option<&str>,
        _cancel: CancellationToken,
    ) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "not implemented"))
    }

    async fn exec(&self, _command: &str, _options: ExecOptions) -> ExecResult<ExecOutput> {
        Err(ExecutionError::new(
            ExecutionErrorCode::Unknown,
            "not implemented",
        ))
    }
}

#[tokio::test]
async fn cleanup_default_impl_is_noop() {
    let env = NoopEnv;
    env.cleanup().await;
}
