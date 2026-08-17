//! Tests for `agent::skills` walker and validation helpers — split out of src
//! (see docs/rust-test-files.md). The src file already has a small inline test
//! module, so these tests are bridged through `skills_walk_tests`.

use std::collections::HashMap;

use super::super::*;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

// ──────────────────────────────────────────────────────────────────────────────────────────
// TestExecutionEnv — in-memory filesystem seam
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct TestEnv {
    infos: HashMap<String, FileInfo>,
    children: HashMap<String, Vec<FileInfo>>,
    contents: HashMap<String, String>,
    canonical: HashMap<String, String>,
    file_info_errors: HashMap<String, FileError>,
}

impl TestEnv {
    fn info(path: &str, kind: FileKind) -> FileInfo {
        let path = path.trim_end_matches('/');
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        FileInfo {
            name,
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

    fn put_canonical(&mut self, path: &str, target: &str) {
        self.canonical.insert(path.to_string(), target.to_string());
    }

    fn dir_with_file(&mut self, dir: &str, file: &str, kind: FileKind) {
        self.put_info(&format!("{dir}/{file}"), kind);
        self.children
            .entry(dir.to_string())
            .or_default()
            .push(Self::info(&format!("{dir}/{file}"), kind));
    }

    fn dir_with_skill(&mut self, dir: &str, body: &str) {
        self.put_info(dir, FileKind::Directory);
        self.dir_with_file(dir, "SKILL.md", FileKind::File);
        self.put_content(
            &format!("{dir}/SKILL.md"),
            format!(
                "---\nname: {}\ndescription: test skill\n---\n{}",
                dir.rsplit('/').next().unwrap_or(dir),
                body
            ),
        );
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
        self.contents
            .get(path)
            .cloned()
            .ok_or_else(|| FileError::new(FileErrorCode::NotFound, "no content").with_path(path))
    }

    async fn read_text_lines(
        &self,
        path: &str,
        _max_lines: Option<usize>,
        _cancel: CancellationToken,
    ) -> FsResult<Vec<String>> {
        Ok(self.read_text_file(path, _cancel).await?.lines().map(str::to_string).collect())
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
        self.children
            .get(path.trim_end_matches('/'))
            .cloned()
            .ok_or_else(|| FileError::new(FileErrorCode::NotFound, "missing dir").with_path(path))
    }

    async fn exists(&self, path: &str, _cancel: CancellationToken) -> FsResult<bool> {
        Ok(self.infos.contains_key(path.trim_end_matches('/')))
    }

    async fn canonical_path(&self, path: &str, _cancel: CancellationToken) -> FsResult<String> {
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

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn env_with_root_skill() -> (TestEnv, String) {
    let mut env = TestEnv::default();
    env.dir_with_skill("/skills/root", "root body");
    (env, "/skills/root".to_string())
}

#[tokio::test]
async fn load_skills_skips_missing_directories_without_diagnostics() {
    let env = TestEnv::default();

    let out = load_skills(&env, &["/does/not/exist"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_skills_skips_non_directory_root() {
    let mut env = TestEnv::default();
    env.put_info("/file.md", FileKind::File);

    let out = load_skills(&env, &["/file.md"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_skills_emits_file_info_diagnostic_for_unknown_error() {
    let mut env = TestEnv::default();
    env.put_info("/root", FileKind::Directory);
    env.put_children("/root", Vec::new());
    env.file_info_errors.insert(
        "/root/.gitignore".into(),
        FileError::new(FileErrorCode::PermissionDenied, "denied").with_path("/root/.gitignore"),
    );

    let out = load_skills(&env, &["/root"], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::FileInfoFailed)
    );
}

#[tokio::test]
async fn load_skills_loads_root_skill_and_ignores_children() {
    let mut env = TestEnv::default();
    let root = "/skills/root";
    env.dir_with_skill(root, "root body");
    // A child SKILL.md should NOT be loaded because the parent has its own SKILL.md.
    env.put_info("/skills/root/child", FileKind::Directory);
    env.put_info("/skills/root/child/SKILL.md", FileKind::File);
    env.put_content(
        "/skills/root/child/SKILL.md",
        "---\nname: child\ndescription: child skill\n---\nchild body",
    );

    let out = load_skills(&env, &[root], cancel()).await;

    assert_eq!(out.skills.len(), 1);
    assert_eq!(out.skills[0].name, "root");
    assert_eq!(out.skills[0].content, "root body");
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_skills_recurses_into_subdirs_and_loads_root_md_files_only_at_root() {
    let mut env = TestEnv::default();
    let root = "/skills";
    env.put_info(root, FileKind::Directory);
    // Root-level direct .md is a skill at the root level.
    env.put_info("/skills/README.md", FileKind::File);
    env.put_content(
        "/skills/README.md",
        "---\nname: readme\ndescription: readme skill\n---\nreadme body",
    );
    // Child directory without its own SKILL.md: its *.md files are ignored,
    // but its subdirectories are still recursed into.
    env.put_info("/skills/child", FileKind::Directory);
    env.put_info("/skills/child/notes.md", FileKind::File);
    env.put_content(
        "/skills/child/notes.md",
        "---\nname: notes\ndescription: notes skill\n---\nnotes body",
    );
    // Grandchild with SKILL.md is found by recursion.
    env.dir_with_skill("/skills/child/grandchild", "grandchild body");

    env.put_children(
        root,
        vec![
            TestEnv::info("/skills/README.md", FileKind::File),
            TestEnv::info("/skills/child", FileKind::Directory),
        ],
    );
    env.put_children(
        "/skills/child",
        vec![
            TestEnv::info("/skills/child/grandchild", FileKind::Directory),
            TestEnv::info("/skills/child/notes.md", FileKind::File),
        ],
    );
    env.put_children(
        "/skills/child/grandchild",
        vec![TestEnv::info(
            "/skills/child/grandchild/SKILL.md",
            FileKind::File,
        )],
    );

    let out = load_skills(&env, &[root], cancel()).await;

    let names: Vec<&str> = out.skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"readme"));
    assert!(!names.contains(&"notes"));
    assert!(names.contains(&"grandchild"));
}

#[tokio::test]
async fn load_skills_skips_skill_with_empty_description() {
    let mut env = TestEnv::default();
    let root = "/skills/root";
    env.put_info(root, FileKind::Directory);
    env.put_info(&format!("{root}/SKILL.md"), FileKind::File);
    env.put_content(
        &format!("{root}/SKILL.md"),
        "---\nname: root\ndescription: \"\"\n---\nbody",
    );
    env.put_children(root, vec![TestEnv::info(&format!("{root}/SKILL.md"), FileKind::File)]);

    let out = load_skills(&env, &[root], cancel()).await;

    assert!(out.skills.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::InvalidMetadata)
    );
}

#[tokio::test]
async fn load_skills_emits_read_and_parse_diagnostics() {
    let mut env = TestEnv::default();
    let root = "/skills/root";
    env.put_info(root, FileKind::Directory);
    env.put_info(&format!("{root}/SKILL.md"), FileKind::File);
    env.put_children(root, vec![TestEnv::info(&format!("{root}/SKILL.md"), FileKind::File)]);
    // Missing content -> ReadFailed.
    let out = load_skills(&env, &[root], cancel()).await;
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::ReadFailed)
    );

    // Invalid YAML -> ParseFailed.
    env.put_content(
        &format!("{root}/SKILL.md"),
        "---\nname: [\n---\nbody",
    );
    let out = load_skills(&env, &[root], cancel()).await;
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == SkillDiagnosticCode::ParseFailed)
    );
}

#[tokio::test]
async fn load_sourced_skills_tags_skill_with_each_source() {
    let (env, root) = env_with_root_skill();

    let out = load_sourced_skills(
        &env,
        &[(&root, "builtin"), (&root, "project")],
        cancel(),
    )
    .await;

    assert_eq!(out.len(), 2);
    assert_eq!(out[0].1, "builtin");
    assert_eq!(out[1].1, "project");
    assert_eq!(out[0].0.name, "root");
}

#[tokio::test]
async fn walker_applies_gitignore_to_root_files() {
    let mut env = TestEnv::default();
    let root = "/skills";
    env.put_info(root, FileKind::Directory);
    env.dir_with_file(root, ".gitignore", FileKind::File);
    env.put_content(&format!("{root}/.gitignore"), "ignored.md\n");
    // A root-level .md file that the ignore file excludes.
    env.put_info("/skills/ignored.md", FileKind::File);
    env.put_content(
        "/skills/ignored.md",
        "---\nname: ignored\ndescription: ignored skill\n---\nignored body",
    );
    // A kept skill in a subdirectory.
    env.dir_with_skill("/skills/kept", "kept body");
    env.put_children(
        root,
        vec![
            TestEnv::info(&format!("{root}/.gitignore"), FileKind::File),
            TestEnv::info("/skills/ignored.md", FileKind::File),
            TestEnv::info("/skills/kept", FileKind::Directory),
        ],
    );

    let out = load_skills(&env, &[root], cancel()).await;

    let names: Vec<&str> = out.skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"kept"));
    assert!(!names.contains(&"ignored"));
}

#[tokio::test]
async fn resolve_kind_follows_symlink_to_file() {
    let mut env = TestEnv::default();
    env.put_info("/link", FileKind::Symlink);
    env.put_canonical("/link", "/real.md");
    env.put_info("/real.md", FileKind::File);

    let mut diagnostics = Vec::new();
    let info = TestEnv::info("/link", FileKind::Symlink);

    let kind = resolve_kind(&env, &info, &mut diagnostics, &cancel()).await;

    assert_eq!(kind, Some(FileKind::File));
    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn resolve_kind_returns_none_for_missing_canonical() {
    let mut env = TestEnv::default();
    env.put_info("/link", FileKind::Symlink);
    // canonical_path will report NotFound.

    let mut diagnostics = Vec::new();
    let info = TestEnv::info("/link", FileKind::Symlink);

    let kind = resolve_kind(&env, &info, &mut diagnostics, &cancel()).await;

    assert_eq!(kind, None);
    assert!(diagnostics.is_empty());
}

#[test]
fn prefix_ignore_pattern_handles_comments_negation_and_root_patterns() {
    assert_eq!(prefix_ignore_pattern("# comment", "sub/"), None);
    assert_eq!(prefix_ignore_pattern("", "sub/"), None);
    assert_eq!(prefix_ignore_pattern("build/", "sub/"), Some("sub/build/".into()));
    assert_eq!(
        prefix_ignore_pattern("!keep.md", "sub/"),
        Some("!sub/keep.md".into())
    );
    assert_eq!(prefix_ignore_pattern("\\!literal", "sub/"), Some("sub/literal".into()));
    assert_eq!(prefix_ignore_pattern("/root.md", ""), Some("root.md".into()));
}

#[test]
fn parse_frontmatter_normalizes_crlf_and_rejects_bad_yaml() {
    let (fm, body) = parse_frontmatter("---\r\nname: crlf-skill\r\ndescription: ok\r\n---\r\nbody line").unwrap();
    assert_eq!(fm.name.as_deref(), Some("crlf-skill"));
    assert_eq!(body, "body line");

    let (fm, body) = parse_frontmatter("---\nname: missing-close\n").unwrap();
    assert!(fm.name.is_none());
    assert!(body.starts_with("---"));

    assert!(parse_frontmatter("---\nname: [\n---\nbody").is_err());
}

#[test]
fn validate_name_checks_length_and_hyphen_rules() {
    assert!(!validate_name(&"a".repeat(65), "root").is_empty());
    assert!(!validate_name("no_underscore", "no_underscore").is_empty());
    assert!(!validate_name("a--b", "a--b").is_empty());
    assert!(validate_name("ok-name", "ok-name").is_empty());
}

#[test]
fn validate_description_checks_length() {
    assert!(!validate_description(&"x".repeat(1025)).is_empty());
    assert!(validate_description("just right").is_empty());
}

#[test]
fn format_skill_invocation_without_extra_instructions_closes_block() {
    let skill = Skill {
        name: "my-skill".into(),
        description: "do".into(),
        file_path: "/skills/my-skill/SKILL.md".into(),
        content: "hello".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };
    let out = format_skill_invocation(&skill, None);
    assert!(out.starts_with("<skill name=\"my-skill\""));
    assert!(out.contains("References are relative to /skills/my-skill."));
    assert!(out.ends_with("</skill>"));
}

#[test]
fn env_path_helpers_normalize_relative_paths() {
    assert_eq!(join_env_path("/a/b/", "/c"), "/a/b/c");
    assert_eq!(dirname_env_path("/a/b/c"), "/a/b");
    assert_eq!(relative_env_path("/root", "/root"), "");
    assert_eq!(relative_env_path("/root", "/root/a"), "a");
    assert_eq!(relative_env_path("/root", "/other/a"), "other/a");
}
