//! Additional mirrored coverage for `templates` — the fake-`ExecutionEnv`
//! error branches of `load_templates` and the user/project precedence of
//! `load_all`.

use async_trait::async_trait;
use theway_core::{
    ExecOptions, ExecOutput, ExecutionEnv, FileError, FileErrorCode, FileInfo, FileKind, FsResult,
    SkillDiagnosticCode,
};
use tokio_util::sync::CancellationToken;

use super::super::*;
use crate::test_env::{EnvGuard, ENV_LOCK};

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn file_info(name: &str, path: &str, kind: FileKind) -> FileInfo {
    FileInfo {
        name: name.to_string(),
        path: path.to_string(),
        kind,
        size: 0,
        mtime_ms: 0,
    }
}

/// In-memory `ExecutionEnv` with per-call results; unused methods are
/// unreachable but must still be implemented for the trait.
struct FakeEnv {
    file_info_result: Option<FsResult<FileInfo>>,
    list_dir_result: Option<FsResult<Vec<FileInfo>>>,
    read_text_file_result: Option<FsResult<String>>,
}

impl FakeEnv {
    fn new() -> Self {
        Self {
            file_info_result: None,
            list_dir_result: None,
            read_text_file_result: None,
        }
    }

    fn with_file_info(mut self, result: FsResult<FileInfo>) -> Self {
        self.file_info_result = Some(result);
        self
    }

    fn with_list_dir(mut self, result: FsResult<Vec<FileInfo>>) -> Self {
        self.list_dir_result = Some(result);
        self
    }

    fn with_read_text_file(mut self, result: FsResult<String>) -> Self {
        self.read_text_file_result = Some(result);
        self
    }
}

#[async_trait]
impl ExecutionEnv for FakeEnv {
    fn cwd(&self) -> &str {
        "/fake"
    }

    async fn absolute_path(&self, _path: &str, _cancel: CancellationToken) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn join_path(&self, _parts: &[&str], _cancel: CancellationToken) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn read_text_file(&self, _path: &str, _cancel: CancellationToken) -> FsResult<String> {
        self.read_text_file_result
            .clone()
            .unwrap_or_else(|| Err(FileError::new(FileErrorCode::Unknown, "unused")))
    }

    async fn read_text_lines(
        &self,
        _path: &str,
        _max_lines: Option<usize>,
        _cancel: CancellationToken,
    ) -> FsResult<Vec<String>> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn read_binary_file(&self, _path: &str, _cancel: CancellationToken) -> FsResult<Vec<u8>> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &[u8],
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn append_file(
        &self,
        _path: &str,
        _content: &[u8],
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn file_info(&self, _path: &str, _cancel: CancellationToken) -> FsResult<FileInfo> {
        self.file_info_result
            .clone()
            .unwrap_or_else(|| Err(FileError::new(FileErrorCode::Unknown, "unused")))
    }

    async fn list_dir(&self, _path: &str, _cancel: CancellationToken) -> FsResult<Vec<FileInfo>> {
        self.list_dir_result
            .clone()
            .unwrap_or_else(|| Err(FileError::new(FileErrorCode::Unknown, "unused")))
    }

    async fn exists(&self, _path: &str, _cancel: CancellationToken) -> FsResult<bool> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn canonical_path(&self, _path: &str, _cancel: CancellationToken) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn create_dir(
        &self,
        _path: &str,
        _recursive: bool,
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn remove(
        &self,
        _path: &str,
        _recursive: bool,
        _force: bool,
        _cancel: CancellationToken,
    ) -> FsResult<()> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn create_temp_dir(
        &self,
        _prefix: Option<&str>,
        _cancel: CancellationToken,
    ) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn create_temp_file(
        &self,
        _prefix: Option<&str>,
        _suffix: Option<&str>,
        _cancel: CancellationToken,
    ) -> FsResult<String> {
        Err(FileError::new(FileErrorCode::Unknown, "unused"))
    }

    async fn exec(&self, _command: &str, _options: ExecOptions) -> theway_core::ExecResult<ExecOutput> {
        Err(theway_core::ExecutionError::new(
            theway_core::ExecutionErrorCode::Unknown,
            "unused",
        ))
    }
}

#[tokio::test]
async fn load_templates_emits_file_info_diagnostic_for_non_not_found_errors() {
    let env = FakeEnv::new().with_file_info(Err(FileError::new(
        FileErrorCode::PermissionDenied,
        "denied",
    )));
    let dir = "/denied";

    let out = load_templates(&env, &[dir], cancel()).await;

    assert!(out.templates.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].code, SkillDiagnosticCode::FileInfoFailed);
    assert_eq!(out.diagnostics[0].path, dir);
}

#[tokio::test]
async fn load_templates_skips_not_found_file_info_errors() {
    let env = FakeEnv::new().with_file_info(Err(FileError::new(
        FileErrorCode::NotFound,
        "missing",
    )));

    let out = load_templates(&env, &["/missing"], cancel()).await;

    assert!(out.templates.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_templates_emits_list_diagnostic() {
    let env = FakeEnv::new()
        .with_file_info(Ok(file_info("templates", "/templates", FileKind::Directory)))
        .with_list_dir(Err(FileError::new(
            FileErrorCode::PermissionDenied,
            "denied",
        )));

    let out = load_templates(&env, &["/templates"], cancel()).await;

    assert!(out.templates.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].code, SkillDiagnosticCode::ListFailed);
    assert_eq!(out.diagnostics[0].path, "/templates");
}

#[tokio::test]
async fn load_templates_emits_read_diagnostic_for_unreadable_md_file() {
    let env = FakeEnv::new()
        .with_file_info(Ok(file_info("templates", "/templates", FileKind::Directory)))
        .with_list_dir(Ok(vec![file_info(
            "broken.md",
            "/templates/broken.md",
            FileKind::File,
        )]))
        .with_read_text_file(Err(FileError::new(
            FileErrorCode::PermissionDenied,
            "denied",
        )));

    let out = load_templates(&env, &["/templates"], cancel()).await;

    assert!(out.templates.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].code, SkillDiagnosticCode::ReadFailed);
    assert_eq!(out.diagnostics[0].path, "/templates/broken.md");
}

#[tokio::test]
async fn load_templates_skips_md_named_directories() {
    let env = FakeEnv::new()
        .with_file_info(Ok(file_info("templates", "/templates", FileKind::Directory)))
        .with_list_dir(Ok(vec![file_info(
            "dir.md",
            "/templates/dir.md",
            FileKind::Directory,
        )]));

    let out = load_templates(&env, &["/templates"], cancel()).await;

    assert!(out.templates.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_all_user_templates_load_and_project_templates_override() {
    // Arrange: explicit `DaemonPaths` base has `shared.md` and `user-only.md`;
    // project `.theway/templates` has its own `shared.md`. The environment is
    // pointed at a poisoned base to prove the explicit paths win.
    let _env_lock = ENV_LOCK.lock().unwrap();
    let poisoned_base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", poisoned_base.path());
    std::fs::create_dir_all(poisoned_base.path().join("templates")).unwrap();
    std::fs::write(
        poisoned_base.path().join("templates/poisoned.md"),
        "---\nname: poisoned\n---\npoisoned body",
    )
    .unwrap();

    let user_base = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let user_templates = user_base.path().join("templates");
    let project_templates = cwd.path().join(".theway").join("templates");
    std::fs::create_dir_all(&user_templates).unwrap();
    std::fs::create_dir_all(&project_templates).unwrap();
    std::fs::write(
        user_templates.join("shared.md"),
        "---\nname: shared\ndescription: user\n---\nuser body",
    )
    .unwrap();
    std::fs::write(
        user_templates.join("user-only.md"),
        "---\nname: user-only\n---\nuser only body",
    )
    .unwrap();
    std::fs::write(
        project_templates.join("shared.md"),
        "---\nname: shared\ndescription: project\n---\nproject body",
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: user_base.path().to_path_buf(),
        home: user_base.path().to_path_buf(),
        work_dir: cwd.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };

    // Act
    let LoadedTemplates {
        templates,
        diagnostics,
    } = load_all(&paths).await;

    // Assert
    assert!(diagnostics.is_empty(), "{:?}", diagnostics);
    let shared = templates
        .iter()
        .find(|t| t.name == "shared")
        .expect("shared template loaded");
    assert_eq!(shared.description.as_deref(), Some("project"));
    assert_eq!(shared.content, "project body");
    assert!(templates.iter().any(|t| t.name == "user-only"));
    assert!(
        !templates.iter().any(|t| t.name == "poisoned"),
        "explicit DaemonPaths must win over THEWAY_DIR: {templates:?}"
    );
}


