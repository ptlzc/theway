//! Integration test for the CLI prompt-template loader (`theway_daemon::templates::load_all`).
//! Verifies the dual-root overlay (`DaemonPaths.base/templates/` user root,
//! `DaemonPaths.work_dir/.theway/templates/` project root, project winning on name
//! collision), frontmatter parsing, diagnostics for broken files, and that loaded
//! templates interpolate via `PromptTemplate::interpolate`.

use std::path::Path;
use std::sync::Mutex;

use tempfile::TempDir;
use theway_daemon::DaemonPaths;
use theway_daemon::templates::{LoadedTemplates, load_all};

static THEWAY_DIR_ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn write(root: &Path, name: &str, frontmatter_desc: &str, body: &str) {
    std::fs::create_dir_all(root).unwrap();
    let content = format!("---\nname: {name}\ndescription: {frontmatter_desc}\n---\n{body}\n");
    std::fs::write(root.join(format!("{name}.md")), content).unwrap();
}

fn paths(base: &Path, work: &Path) -> DaemonPaths {
    DaemonPaths {
        base: base.to_path_buf(),
        home: base.to_path_buf(),
        work_dir: work.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    }
}

#[tokio::test]
async fn loads_templates_from_dual_roots_with_project_winning() {
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let poisoned = TempDir::new().unwrap();
    let _env = EnvGuard::set("THEWAY_DIR", poisoned.path());
    write(
        &poisoned.path().join("templates"),
        "poisoned",
        "poison",
        "Must not load",
    );

    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    write(
        &base.path().join("templates"),
        "shared",
        "user",
        "User body {{var}}",
    );
    write(
        &cwd.path().join(".theway").join("templates"),
        "shared",
        "project",
        "Project body {{var}}",
    );
    write(
        &base.path().join("templates"),
        "only-user",
        "user-only",
        "Only user",
    );

    let LoadedTemplates {
        templates,
        diagnostics,
    } = load_all(&paths(base.path(), cwd.path())).await;
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"shared"));
    assert!(names.contains(&"only-user"));
    assert!(
        !names.contains(&"poisoned"),
        "explicit paths must win: {names:?}"
    );

    let shared = templates.iter().find(|t| t.name == "shared").unwrap();
    assert_eq!(shared.description.as_deref(), Some("project"));
    assert!(shared.content.contains("Project body"));

    // Interpolation round-trip on the loaded template.
    let mut vars = serde_json::Map::new();
    vars.insert("var".into(), serde_json::json!("world"));
    assert_eq!(shared.interpolate(&vars), "Project body world");
}

#[tokio::test]
async fn frontmatter_name_overrides_file_stem() {
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let poisoned = TempDir::new().unwrap();
    let _env = EnvGuard::set("THEWAY_DIR", poisoned.path());
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    let templates_dir = base.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    // File stem is `myname.md`, frontmatter names it `greet`.
    std::fs::write(
        templates_dir.join("myname.md"),
        "---\nname: greet\ndescription: greeting\n---\nHi {{who}}",
    )
    .unwrap();

    let LoadedTemplates {
        templates,
        diagnostics,
    } = load_all(&paths(base.path(), cwd.path())).await;
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].name, "greet");
    assert_eq!(templates[0].description.as_deref(), Some("greeting"));
}

#[tokio::test]
async fn missing_dirs_produce_no_diagnostics() {
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let poisoned = TempDir::new().unwrap();
    let _env = EnvGuard::set("THEWAY_DIR", poisoned.path());
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    let LoadedTemplates {
        templates,
        diagnostics,
    } = load_all(&paths(base.path(), cwd.path())).await;
    assert!(templates.is_empty());
    assert!(diagnostics.is_empty(), "{:#?}", diagnostics);
}

#[tokio::test]
async fn broken_frontmatter_reports_parse_diagnostic() {
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let poisoned = TempDir::new().unwrap();
    let _env = EnvGuard::set("THEWAY_DIR", poisoned.path());
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    let templates_dir = base.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(
        templates_dir.join("broken.md"),
        "---\nname: [unclosed\n---\nBody",
    )
    .unwrap();

    let LoadedTemplates {
        templates,
        diagnostics,
    } = load_all(&paths(base.path(), cwd.path())).await;
    assert!(templates.is_empty());
    assert!(
        diagnostics
            .iter()
            .any(|d| matches!(d.code, theway_core::SkillDiagnosticCode::ParseFailed))
    );
}
