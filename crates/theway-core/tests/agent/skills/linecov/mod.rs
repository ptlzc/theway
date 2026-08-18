//! Additional line-coverage tests for `agent::skills` (see docs/rust-test-files.md).

use std::collections::HashMap;

use super::super::*;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct TestEnv {
    infos: HashMap<String, FileInfo>,
    children: HashMap<String, Vec<FileInfo>>,
    contents: HashMap<String, String>,
    file_info_errors: HashMap<String, FileError>,
    list_dir_errors: HashMap<String, FileError>,
    read_errors: HashMap<String, FileError>,
}

impl TestEnv {
    fn info(path: &str, kind: FileKind) -> FileInfo {
        let path = path.trim_end_matches('/');
        FileInfo {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            kind,
            size: 0,
            mtime_ms: 0,
        }
    }

    fn put_info(&mut self, path: &str, kind: FileKind) {
        let info = Self::info(path, kind);
        self.infos.insert(info.path.clone(), info);
    }

    fn put_children(&mut self, dir: &str, children: Vec<FileInfo>) {
        self.children.insert(dir.trim_end_matches('/').to_string(), children);
    }

    fn put_content(&mut self, path: &str, content: impl Into<String>) {
        self.contents.insert(path.to_string(), content.into());
    }
}

#[async_trait]
impl ExecutionEnv for TestEnv {
    fn cwd(&self) -> &str {
        "/"
    }

    async fn absolute_path(&self, path: &str, _cancel: CancellationToken) -> FsResult<String> {
        Ok(path.to_string())
    }

    async fn join_path(&self, parts: &[&str], _cancel: CancellationToken) -> FsResult<String> {
        Ok(parts.join("/"))
    }

    async fn read_text_file(&self, path: &str, _cancel: CancellationToken) -> FsResult<String> {
        if let Some(err) = self.read_errors.get(path) {
            return Err(err.clone());
        }
        self.contents
            .get(path)
            .cloned()
            .ok_or_else(|| FileError::new(FileErrorCode::NotFound, "no content").with_path(path))
    }

    async fn read_text_lines(
        &self,
        path: &str,
        max_lines: Option<usize>,
        cancel: CancellationToken,
    ) -> FsResult<Vec<String>> {
        Ok(self
            .read_text_file(path, cancel)
            .await?
            .lines()
            .take(max_lines.unwrap_or(usize::MAX))
            .map(str::to_string)
            .collect())
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
        if let Some(err) = self.file_info_errors.get(path.trim_end_matches('/')) {
            return Err(err.clone());
        }
        self.infos
            .get(path.trim_end_matches('/'))
            .cloned()
            .ok_or_else(|| FileError::new(FileErrorCode::NotFound, "missing").with_path(path))
    }

    async fn list_dir(&self, path: &str, _cancel: CancellationToken) -> FsResult<Vec<FileInfo>> {
        if let Some(err) = self.list_dir_errors.get(path.trim_end_matches('/')) {
            return Err(err.clone());
        }
        self.children
            .get(path.trim_end_matches('/'))
            .cloned()
            .ok_or_else(|| FileError::new(FileErrorCode::NotFound, "missing dir").with_path(path))
    }

    async fn exists(&self, path: &str, _cancel: CancellationToken) -> FsResult<bool> {
        Ok(self.infos.contains_key(path.trim_end_matches('/')))
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

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

#[tokio::test]
async fn load_skills_emits_file_info_diagnostic_for_root_dir() {
    let mut env = TestEnv::default();
    env.file_info_errors.insert(
        "/root".into(),
        FileError::new(FileErrorCode::PermissionDenied, "denied").with_path("/root"),
    );

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::FileInfoFailed && d.path == "/root")
    );
}

#[tokio::test]
async fn walk_one_emits_list_failed_diagnostic() {
    let mut env = TestEnv::default();
    env.put_info("/root", FileKind::Directory);
    env.list_dir_errors.insert(
        "/root".into(),
        FileError::new(FileErrorCode::PermissionDenied, "denied").with_path("/root"),
    );

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::ListFailed && d.path == "/root")
    );
}

#[tokio::test]
async fn walk_one_skips_skill_md_when_it_is_not_a_file() {
    let mut env = TestEnv::default();
    env.put_info("/root", FileKind::Directory);
    env.put_info("/root/SKILL.md", FileKind::Directory);
    env.put_children("/root", vec![TestEnv::info("/root/SKILL.md", FileKind::Directory)]);
    env.put_children("/root/SKILL.md", Vec::new());

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn walk_one_skips_ignored_skill_md() {
    let mut env = TestEnv::default();
    env.put_info("/root", FileKind::Directory);
    env.put_info("/root/.gitignore", FileKind::File);
    env.put_content("/root/.gitignore", "SKILL.md\n");
    env.put_info("/root/SKILL.md", FileKind::File);
    env.put_content(
        "/root/SKILL.md",
        "---\nname: root\ndescription: test\n---\nbody",
    );
    env.put_children(
        "/root",
        vec![
            TestEnv::info("/root/.gitignore", FileKind::File),
            TestEnv::info("/root/SKILL.md", FileKind::File),
        ],
    );

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn walk_one_skips_non_file_ignore_rule() {
    let mut env = TestEnv::default();
    env.put_info("/root", FileKind::Directory);
    env.put_info("/root/.gitignore", FileKind::Directory);
    env.put_children(
        "/root",
        vec![TestEnv::info("/root/.gitignore", FileKind::Directory)],
    );

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn walk_one_emits_read_failed_for_ignore_file() {
    let mut env = TestEnv::default();
    env.put_info("/root", FileKind::Directory);
    env.put_info("/root/.gitignore", FileKind::File);
    env.put_children(
        "/root",
        vec![TestEnv::info("/root/.gitignore", FileKind::File)],
    );
    env.read_errors.insert(
        "/root/.gitignore".into(),
        FileError::new(FileErrorCode::PermissionDenied, "denied").with_path("/root/.gitignore"),
    );

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::ReadFailed && d.path == "/root/.gitignore")
    );
}
