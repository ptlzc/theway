use std::path::Path;
use std::sync::Arc;

use super::{faux_model, test_factory};
use crate::runtime_storage::SessionRepository;
use crate::test_env::{ENV_LOCK, EnvGuard};
use tempfile::TempDir;
use theway_core::ThinkingLevel;

#[tokio::test]
async fn build_for_work_dir_canonicalizes_and_loads_cwd_scoped_resources() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let real = TempDir::new().unwrap();
    let sub = real.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let project = real.path().join(".theway");
    let skill = project.join("skills").join("project-skill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: project-skill\ndescription: project skill\n---\nproject\n",
    )
    .unwrap();
    let templates = project.join("templates");
    std::fs::create_dir_all(&templates).unwrap();
    std::fs::write(
        templates.join("review.md"),
        "---\nname: review\n---\nProject template\n",
    )
    .unwrap();
    let extension = project.join("extensions").join("project-ext");
    std::fs::create_dir_all(&extension).unwrap();
    std::fs::write(
        extension.join("theway-extension.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "project-ext",
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        extension.join("index.js"),
        "export default defineExtension((api) => api);",
    )
    .unwrap();
    std::fs::write(
        project.join("hooks.toml"),
        "[[hook]]\nevent = \"turn_end\"\ncommand = \"echo project\"\n",
    )
    .unwrap();
    std::fs::write(real.path().join("probe.txt"), "probe").unwrap();

    let base = TempDir::new().unwrap();
    std::fs::write(base.path().join("hooks.toml"), "allow_project_hooks = true\n").unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(theway_storage::sqlite_repo::SqliteSessionRepo::new(
        base.path().join("repo"),
    ));
    let storage = crate::runtime_storage::local_runtime_storage();
    let requested = sub.join("..");
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: requested.clone(),
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
    };

    let ctx = crate::orchestration::SessionExecutionContext::build_for_work_dir(
        "context-canonical",
        requested,
        repo,
        storage,
        paths,
        faux_model(),
        ThinkingLevel::High,
        &[],
        &[],
        true,
    )
    .await
    .unwrap();

    let canonical = real.path().canonicalize().unwrap();
    assert_eq!(ctx.cwd, canonical);
    assert_eq!(ctx.paths.work_dir, canonical);
    assert_eq!(
        ctx.executor.read_file(Path::new("probe.txt")).await.unwrap(),
        "probe"
    );
    assert!(ctx.resources.skills.iter().any(|s| s.name == "project-skill"));
    assert!(ctx.resources.templates.iter().any(|t| t.name == "review"));
    assert_eq!(
        ctx.hooks.loaded_hooks("context-canonical", None, None).runner.len(),
        1
    );
    assert!(ctx
        .extension_resources
        .runtime_extension_packages
        .read()
        .selected_packages()
        .iter()
        .any(|p| p.manifest().id == "project-ext"));
}

#[tokio::test]
async fn context_thinking_is_used_when_building_runtime() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(theway_storage::sqlite_repo::SqliteSessionRepo::new(
        repo_root.path().join("repo"),
    ));
    let store = repo.create(work_dir.path()).await.unwrap();
    let session_id = store
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    let (factory, storage, _state) = test_factory();
    assert_eq!(factory.thinking, ThinkingLevel::Off);
    let base = TempDir::new().unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: work_dir.path().to_path_buf(),
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let ctx = crate::orchestration::SessionExecutionContext::build_for_work_dir(
        session_id.clone(),
        work_dir.path().to_path_buf(),
        repo,
        storage,
        paths,
        faux_model(),
        ThinkingLevel::High,
        &[],
        &[],
        false,
    )
    .await
    .unwrap();

    let runtime = factory.build_opened(&ctx, store, false).await.unwrap();
    assert_eq!(
        runtime.harness.agent().state().thinking_level,
        Some(ThinkingLevel::High)
    );
}
