// ─────────────────────────────────────────────────────────────────────────────────────────
// execution context
// ─────────────────────────────────────────────────────────────────────────────────────────

use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::test_env::{ENV_LOCK, EnvGuard};

/// Controller mode (issues #95/#96): a session built AFTER the TUI provisioned
/// the catalog (daemon restart + restore, session switch, lazy build) must
/// read the provisioned skill/template slots — `Configure` only targets the
/// active session's harness, so the build path is the only way new sessions
/// get the catalog.
#[tokio::test]
async fn controller_mode_build_reads_provisioned_skill_and_template_slots() {
    // hooks::load inside build reads THEWAY_DIR — isolate it.
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let paths = crate::DaemonPaths {
        base: home.path().join("base"),
        home: home.path().to_path_buf(),
        work_dir: home.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let paths = paths.with_work_dir(work_dir.path());
    // `false` = controller-provisioned: no disk scan, catalog comes from the slots.
    let resources = SessionProjectResources::load(&paths, &[], &[], false)
        .await
        .unwrap();
    *resources.provisioned_skills.write().unwrap() = vec![theway_core::Skill {
        name: "provisioned-skill".into(),
        description: "from the controller".into(),
        file_path: "/tmp/provisioned-skill/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: theway_core::SkillSource::User,
    }];
    *resources.provisioned_templates.write().unwrap() = vec![theway_core::PromptTemplate {
        name: "provisioned-template".into(),
        description: Some("from the controller".into()),
        file_path: "/tmp/provisioned-template.md".into(),
        content: "template body".into(),
    }];
    let hooks = SessionHookResources::load(&paths, false).await;
    let ctx = SessionExecutionContext::new(
        "test-controller",
        work_dir.path().to_path_buf(),
        std::sync::Arc::new(repo),
        storage,
        paths,
        crate::executor::executor_for_cwd(work_dir.path().to_path_buf()),
        theway_core::executor::ExecutorKind::Local,
        faux_model(),
        theway_core::ThinkingLevel::Off,
        resources,
        SessionMcpResources::default(),
        hooks,
    );
    let runtime = factory
        .build(&ctx, &id)
        .await
        .expect("controller-mode session builds");
    assert!(
        runtime
            .harness
            .skills()
            .iter()
            .any(|skill| skill.name == "provisioned-skill"),
        "build must read the provisioned skill slot"
    );
    assert!(
        runtime
            .harness
            .templates()
            .iter()
            .any(|template| template.name == "provisioned-template"),
        "build must read the provisioned template slot"
    );
}

#[tokio::test]
async fn build_uses_explicit_context_cwd_and_registers_session_ownership() {
    // hooks::load inside build reads THEWAY_DIR — isolate it.
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let recorded_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, recorded_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let ctx = session_context(work_dir.path(), repo, storage, &_state.path().join("base")).await;
    ctx.transcript_store.save(&theway_core::multiagent::jobs::JobTranscript {
        job_id: "job-1",
        run_id: Some("run-1"),
        node_id: Some("node-1"),
        messages: &[serde_json::json!({ "text": "persisted" })],
    });
    let runtime = factory
        .build(&ctx, &id)
        .await
        .expect("session in the context's work_dir builds");
    let prompt = runtime.harness.system_prompt();
    assert!(
        prompt.contains(&format!(
            "Current working directory: {}",
            work_dir.path().canonicalize().unwrap().display()
        )),
        "runtime must use the explicit context cwd: {prompt}"
    );
    assert!(
        !prompt.contains(&recorded_dir.path().display().to_string()),
        "stored metadata must not override the explicit context cwd: {prompt}"
    );
    let registered = factory.services.session_execution.get_context(&id).unwrap();
    let messages = factory
        .subagent_registry
        .node_messages_for_session(Some(&id), "run-1", "node-1")
        .unwrap();
    assert_eq!(runtime.session_id, id);
    assert_eq!(registered.session_id, id);
    assert_eq!(registered.cwd, work_dir.path().canonicalize().unwrap());
    assert_eq!(messages[0]["text"], "persisted");
    assert!(format!("{:?}", factory.dag_engine).contains("launcher: false"));
}

#[tokio::test]
async fn build_one_builder_serves_two_cwd_contexts() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_a = TempDir::new().unwrap();
    let work_b = TempDir::new().unwrap();
    let base_a = TempDir::new().unwrap();
    let base_b = TempDir::new().unwrap();
    for (base, memory) in [
        (&base_a, "memory alpha"),
        (&base_b, "memory beta"),
    ] {
        write_memory(base.path(), memory);
    }
    #[cfg(feature = "local")]
    for (work, skill, template) in [
        (&work_a, "alpha-skill", "Template A"),
        (&work_b, "beta-skill", "Template B"),
    ] {
        let root = work.path().join(".theway");
        write_skill(&root, skill);
        write_template(&root, "review", template);
    }

    let repo_root_a = TempDir::new().unwrap();
    let repo_root_b = TempDir::new().unwrap();
    let repo_a = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_a.path());
    let repo_b = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_b.path());
    let id_a = create_session_with_cwd(&repo_a, work_a.path().to_str().unwrap()).await;
    let id_b = create_session_with_cwd(&repo_b, work_b.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let ctx_a = session_context(work_a.path(), repo_a, storage.clone(), base_a.path()).await;
    let ctx_b = session_context(work_b.path(), repo_b, storage, base_b.path()).await;

    let runtime_a = factory
        .build(&ctx_a, &id_a)
        .await
        .expect("first cwd context builds");
    let runtime_b = factory
        .build(&ctx_b, &id_b)
        .await
        .expect("second cwd context builds");

    let prompt_a = runtime_a.harness.system_prompt();
    let prompt_b = runtime_b.harness.system_prompt();
    assert!(
        prompt_a.contains(&format!(
            "Current working directory: {}",
            work_a.path().canonicalize().unwrap().display()
        )),
        "runtime A must use work_a: {prompt_a}"
    );
    assert!(
        prompt_b.contains(&format!(
            "Current working directory: {}",
            work_b.path().canonicalize().unwrap().display()
        )),
        "runtime B must use work_b: {prompt_b}"
    );

    #[cfg(feature = "local")]
    {
        assert!(has_skill(&runtime_a, "alpha-skill"));
        assert!(!has_skill(&runtime_a, "beta-skill"));
        assert!(has_skill(&runtime_b, "beta-skill"));
        assert!(!has_skill(&runtime_b, "alpha-skill"));
        assert_eq!(template_body(&runtime_a, "review"), "Template A");
        assert_eq!(template_body(&runtime_b, "review"), "Template B");

        write_skill(&work_a.path().join(".theway"), "reloaded-skill");
        runtime_a.harness.reload_skills_from_disk().await.unwrap();
        assert!(has_skill(&runtime_a, "reloaded-skill"));
        assert!(!has_skill(&runtime_b, "reloaded-skill"));
    }

    assert!(prompt_a.contains("memory alpha"));
    assert!(!prompt_a.contains("memory beta"));
    assert!(prompt_b.contains("memory beta"));
    assert!(!prompt_b.contains("memory alpha"));
}

#[cfg(feature = "local")]
fn write_skill(root: &Path, name: &str) {
    let dir = root.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name}\n---\n{name}\n"),
    )
    .unwrap();
}

#[cfg(feature = "local")]
fn write_template(root: &Path, name: &str, body: &str) {
    let dir = root.join("templates");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\n---\n{body}"),
    )
    .unwrap();
}

fn write_memory(base: &Path, body: &str) {
    let dir = base.join("memory");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("memory.md"), body).unwrap();
}

#[cfg(feature = "local")]
fn has_skill(runtime: &super::super::SessionRuntime, name: &str) -> bool {
    runtime.harness.skills().iter().any(|s| s.name == name)
}

#[cfg(feature = "local")]
fn template_body(runtime: &super::super::SessionRuntime, name: &str) -> String {
    runtime
        .harness
        .templates()
        .iter()
        .find(|t| t.name == name)
        .unwrap()
        .content
        .clone()
}
