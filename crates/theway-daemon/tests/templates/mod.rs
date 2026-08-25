//! Mirrored tests for `templates` — split out of src (see docs/rust-test-files.md).
//!
//! The inline `mod tests` covers the golden frontmatter case; this bridged
//! suite covers the rest of `parse_frontmatter`, the private directory loader
//! `load_templates`, and `load_all`'s project-root half inside the lib target.

use tempfile::TempDir;
use theway_core::SkillDiagnosticCode;
use tokio_util::sync::CancellationToken;

use super::super::*;

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn write_template(root: &std::path::Path, name: &str, content: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join(name), content).unwrap();
}

fn daemon_paths(base: &std::path::Path, work: &std::path::Path) -> crate::DaemonPaths {
    crate::DaemonPaths {
        base: base.to_path_buf(),
        home: base.to_path_buf(),
        work_dir: work.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    }
}

#[test]
fn parse_frontmatter_without_frontmatter_returns_defaults_and_normalized_body() {
    let raw = "just a body\r\nwith windows line endings";

    let (fm, body) = parse_frontmatter(raw).unwrap();

    assert!(fm.name.is_none());
    assert!(fm.description.is_none());
    assert_eq!(body, "just a body\nwith windows line endings");
}

#[test]
fn parse_frontmatter_with_unclosed_delimiter_returns_whole_content() {
    let raw = "---\nname: review\nbody text";

    let (fm, body) = parse_frontmatter(raw).unwrap();

    assert!(fm.name.is_none());
    assert_eq!(body, raw);
}

#[test]
fn parse_frontmatter_rejects_invalid_yaml() {
    let raw = "---\nname: [unclosed\n---\nBody";

    let err = parse_frontmatter(raw).expect_err("invalid yaml must fail");

    assert!(err.contains("yaml:"), "{err}");
}

#[test]
fn parse_frontmatter_normalizes_crlf_delimiters() {
    let raw = "---\r\nname: review\r\ndescription: code review\r\n---\r\nBody {{var}}";

    let (fm, body) = parse_frontmatter(raw).unwrap();

    assert_eq!(fm.name.as_deref(), Some("review"));
    assert_eq!(fm.description.as_deref(), Some("code review"));
    assert_eq!(body, "Body {{var}}");
}

#[tokio::test]
async fn load_templates_skips_missing_dirs_silently() {
    let dir = TempDir::new().unwrap();
    let env = NativeEnv::new(dir.path().to_string_lossy().to_string());
    let missing = dir.path().join("missing").to_string_lossy().to_string();

    let out = load_templates(&env, &[&missing], cancel()).await;

    assert!(out.templates.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_templates_loads_md_files_and_ignores_non_md_and_subdirs() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("templates");
    write_template(
        &root,
        "greet.md",
        "---\nname: hello\ndescription: greeting\n---\nHi {{who}}",
    );
    write_template(&root, "notes.txt", "not a template");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    write_template(&root.join("nested"), "inner.md", "not loaded");
    let env = NativeEnv::new(dir.path().to_string_lossy().to_string());
    let root_str = root.to_string_lossy().to_string();

    let out = load_templates(&env, &[&root_str], cancel()).await;

    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    assert_eq!(out.templates.len(), 1);
    let tpl = &out.templates[0];
    assert_eq!(tpl.name, "hello");
    assert_eq!(tpl.description.as_deref(), Some("greeting"));
    assert_eq!(tpl.content, "Hi {{who}}");
    assert_eq!(tpl.file_path, root.join("greet.md").to_string_lossy().to_string());
}

#[tokio::test]
async fn load_templates_skips_non_directory_paths() {
    let dir = TempDir::new().unwrap();
    write_template(dir.path(), "file.md", "not a dir");
    let env = NativeEnv::new(dir.path().to_string_lossy().to_string());
    let file = dir.path().join("file.md").to_string_lossy().to_string();

    let out = load_templates(&env, &[&file], cancel()).await;

    assert!(out.templates.is_empty());
    assert!(out.diagnostics.is_empty());
}

#[tokio::test]
async fn load_templates_emits_parse_diagnostic_for_broken_frontmatter() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("templates");
    write_template(&root, "broken.md", "---\nname: [unclosed\n---\nBody");
    let env = NativeEnv::new(dir.path().to_string_lossy().to_string());
    let root_str = root.to_string_lossy().to_string();

    let out = load_templates(&env, &[&root_str], cancel()).await;

    assert!(out.templates.is_empty());
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].code, SkillDiagnosticCode::ParseFailed);
    assert_eq!(out.diagnostics[0].path, root.join("broken.md").to_string_lossy().to_string());
}

#[tokio::test]
async fn load_templates_uses_file_stem_when_frontmatter_has_no_name() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("templates");
    write_template(&root, "stem.md", "---\ndescription: no name here\n---\nBody");
    let env = NativeEnv::new(dir.path().to_string_lossy().to_string());
    let root_str = root.to_string_lossy().to_string();

    let out = load_templates(&env, &[&root_str], cancel()).await;

    assert_eq!(out.templates.len(), 1);
    assert_eq!(out.templates[0].name, "stem");
}

#[tokio::test]
async fn load_all_loads_project_templates_from_cwd() {
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let project_root = cwd.path().join(".theway").join("templates");
    write_template(
        &project_root,
        "project.md",
        "---\nname: project-only\ndescription: from cwd\n---\nProject body",
    );

    let LoadedTemplates {
        templates,
        diagnostics: _,
    } = load_all(&daemon_paths(base.path(), cwd.path())).await;

    assert!(
        templates.iter().any(|t| t.name == "project-only"),
        "project template should load: {templates:?}"
    );
}
