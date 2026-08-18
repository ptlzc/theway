//! Extra tests for `agent::skills` — bridged through `skills_extra_tests`
//! because the existing test module was already occupied.

use std::collections::HashMap;

use super::super::*;
use async_trait::async_trait;
use ignore::gitignore::GitignoreBuilder;
use tokio_util::sync::CancellationToken;

// Minimal env for merge/ignore/resolve_kind edge cases. The heavy walker
// coverage lives in `tests/agent/skills/mod.rs`; this env only implements the
// seams those tests don't reach.
#[derive(Default)]
struct TinyEnv {
    infos: HashMap<String, FileInfo>,
    canonical: HashMap<String, String>,
    file_info_errors: HashMap<String, FileError>,
    canonical_errors: HashMap<String, FileError>,
}

impl TinyEnv {
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
}

#[async_trait]
impl ExecutionEnv for TinyEnv {
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
        Err(FileError::new(FileErrorCode::NotFound, "no content").with_path(path))
    }

    async fn read_text_lines(
        &self,
        path: &str,
        _max_lines: Option<usize>,
        _cancel: CancellationToken,
    ) -> FsResult<Vec<String>> {
        Err(FileError::new(FileErrorCode::NotFound, "no content").with_path(path))
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

    async fn list_dir(&self, _path: &str, _cancel: CancellationToken) -> FsResult<Vec<FileInfo>> {
        Ok(Vec::new())
    }

    async fn exists(&self, path: &str, _cancel: CancellationToken) -> FsResult<bool> {
        Ok(self.infos.contains_key(path.trim_end_matches('/')))
    }

    async fn canonical_path(&self, path: &str, _cancel: CancellationToken) -> FsResult<String> {
        if let Some(err) = self.canonical_errors.get(path.trim_end_matches('/')) {
            return Err(err.clone());
        }
        self.canonical
            .get(path)
            .cloned()
            .ok_or_else(|| FileError::new(FileErrorCode::NotFound, "no canonical").with_path(path))
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

#[test]
fn merge_ignores_keeps_base_when_extra_empty() {
    let root = "/r";
    let mut base = GitignoreBuilder::new(root);
    base.add_line(None, "*.md").unwrap();
    let base = base.build().unwrap();
    let extra = GitignoreBuilder::new(root).build().unwrap();

    let merged = merge_ignores(&base, extra, root);

    assert!(merged.matched("a.md", false).is_ignore());
    assert!(!merged.matched("a.rs", false).is_ignore());
}

#[test]
fn merge_ignores_prefers_extra_when_non_empty() {
    let root = "/r";
    let mut base = GitignoreBuilder::new(root);
    base.add_line(None, "*.md").unwrap();
    let base = base.build().unwrap();
    let mut extra = GitignoreBuilder::new(root);
    extra.add_line(None, "*.rs").unwrap();
    let extra = extra.build().unwrap();

    let merged = merge_ignores(&base, extra, root);

    assert!(!merged.matched("a.md", false).is_ignore());
    assert!(merged.matched("a.rs", false).is_ignore());
}

#[test]
fn is_ignored_returns_false_for_empty_relative_path() {
    let root = "/r";
    let env = TinyEnv::default();
    let ignore = GitignoreBuilder::new(root).build().unwrap();
    let walker = Walker {
        env: &env,
        root: root.to_string(),
        cancel: CancellationToken::new(),
        ignore,
    };

    assert!(!walker.is_ignored("", false));
}

#[tokio::test]
async fn resolve_kind_returns_none_for_symlink_to_symlink() {
    let mut env = TinyEnv::default();
    env.infos.insert("/link".into(), TinyEnv::info("/link", FileKind::Symlink));
    env.canonical.insert("/link".into(), "/link2".into());
    env.infos.insert("/link2".into(), TinyEnv::info("/link2", FileKind::Symlink));

    let info = TinyEnv::info("/link", FileKind::Symlink);
    let mut diagnostics = Vec::new();

    let kind = resolve_kind(&env, &info, &mut diagnostics, &CancellationToken::new()).await;

    assert_eq!(kind, None);
    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn resolve_kind_reports_non_notfound_canonical_error() {
    let mut env = TinyEnv::default();
    env.infos.insert("/link".into(), TinyEnv::info("/link", FileKind::Symlink));
    env.canonical_errors.insert(
        "/link".into(),
        FileError::new(FileErrorCode::PermissionDenied, "denied").with_path("/link"),
    );

    let info = TinyEnv::info("/link", FileKind::Symlink);
    let mut diagnostics = Vec::new();

    let kind = resolve_kind(&env, &info, &mut diagnostics, &CancellationToken::new()).await;

    assert_eq!(kind, None);
    assert!(diagnostics.iter().any(|d| d.code == SkillDiagnosticCode::FileInfoFailed));
}

#[tokio::test]
async fn resolve_kind_reports_non_notfound_canonical_file_info_error() {
    let mut env = TinyEnv::default();
    env.infos.insert("/link".into(), TinyEnv::info("/link", FileKind::Symlink));
    env.canonical.insert("/link".into(), "/real".into());
    env.file_info_errors.insert(
        "/real".into(),
        FileError::new(FileErrorCode::PermissionDenied, "denied").with_path("/real"),
    );

    let info = TinyEnv::info("/link", FileKind::Symlink);
    let mut diagnostics = Vec::new();

    let kind = resolve_kind(&env, &info, &mut diagnostics, &CancellationToken::new()).await;

    assert_eq!(kind, None);
    assert!(diagnostics.iter().any(|d| d.code == SkillDiagnosticCode::FileInfoFailed));
}

#[test]
fn prefix_ignore_pattern_handles_negation_without_prefix() {
    assert_eq!(prefix_ignore_pattern("!keep.md", ""), Some("!keep.md".into()));
    assert_eq!(prefix_ignore_pattern("\\!keep.md", ""), Some("keep.md".into()));
    assert_eq!(prefix_ignore_pattern("# comment", ""), None);
}

#[tokio::test]
async fn load_sourced_skills_with_empty_inputs_returns_empty() {
    let env = TinyEnv::default();

    let out = load_sourced_skills::<&str>(&env, &[], CancellationToken::new()).await;

    assert!(out.is_empty());
}

#[tokio::test]
async fn resolve_kind_returns_file_or_directory_without_canonical_lookup() {
    let env = TinyEnv::default();
    let mut diagnostics = Vec::new();

    let file = TinyEnv::info("/file.md", FileKind::File);
    assert_eq!(
        resolve_kind(&env, &file, &mut diagnostics, &CancellationToken::new()).await,
        Some(FileKind::File)
    );

    let dir = TinyEnv::info("/dir", FileKind::Directory);
    assert_eq!(
        resolve_kind(&env, &dir, &mut diagnostics, &CancellationToken::new()).await,
        Some(FileKind::Directory)
    );
    assert!(diagnostics.is_empty());
}
