//! `/skills install` and `/skills remove` preview/confirm flows over the local FS.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, SkillSource,
};

use super::super::helpers::*;
// Only the two `local`-gated preview flows below take the auth env lock.
#[cfg(feature = "local")]
use crate::auth;
use crate::commands;

// `/skills install` writes through the local FS (NativeEnv-backed loader mirror); local-only (issue #64).
#[cfg(feature = "local")]
#[tokio::test]
async fn dispatch_skills_install_previews_then_confirms_without_body_echo() {
    let _auth_guard = auth::ENV_LOCK.lock().unwrap();
    let _env_guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let _output_guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let source_dir = temp.path().join("incoming");
    tokio::fs::create_dir_all(&source_dir).await.unwrap();
    let source_path = source_dir.join("SKILL.md");
    tokio::fs::write(
        &source_path,
        "---\nname: db9\ndescription: DB9 helper\n---\nSECRET SKILL BODY\n",
    )
    .await
    .unwrap();

    let harness = harness_with_disk_skill_reload(temp.path(), Vec::new());
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
    };

    let capture = OutputCapture::install();
    let outcome = commands::dispatch(
        &format!("/skills install {}", source_path.display()),
        &registry,
        &ctx,
    )
    .await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(
        harness.skills().is_empty(),
        "preview should not mutate catalog"
    );
    let text = capture.text();
    assert!(text.contains("skill install preview: db9"), "{text}");
    assert!(text.contains("/skills install --confirm"), "{text}");
    assert!(!text.contains("SECRET SKILL BODY"), "{text}");

    let outcome = commands::dispatch(
        &format!("/skills install --confirm {}", source_path.display()),
        &registry,
        &ctx,
    )
    .await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    let skills = harness.skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "db9");
    let text = capture.text();
    assert!(text.contains("installed skill 'db9'"), "{text}");
    assert!(!text.contains("SECRET SKILL BODY"), "{text}");
}

// `/skills remove` deletes through the local FS; local-only (issue #64).
#[cfg(feature = "local")]
#[tokio::test]
async fn dispatch_skills_remove_previews_then_confirms_user_skill() {
    let _auth_guard = auth::ENV_LOCK.lock().unwrap();
    let _env_guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let _output_guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let skill_dir = temp.path().join("skills").join("db9");
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: db9\ndescription: DB9 helper\n---\nSECRET SKILL BODY\n",
    )
    .await
    .unwrap();
    let harness =
        harness_with_disk_skill_reload(temp.path(), vec![user_skill_at(temp.path(), "db9", false)]);
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
    };

    let capture = OutputCapture::install();
    let outcome = commands::dispatch("/skills remove db9", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(skill_dir.exists(), "preview should not remove files");
    let text = capture.text();
    assert!(text.contains("skill remove preview: db9 (user)"), "{text}");
    assert!(!text.contains("SECRET SKILL BODY"), "{text}");

    let outcome = commands::dispatch("/skills remove --confirm db9", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(!skill_dir.exists(), "confirm should remove user skill dir");
    assert!(
        harness.skills().iter().all(|s| s.name != "db9"),
        "reload should drop removed skill"
    );
    let text = capture.text();
    assert!(text.contains("removed skill 'db9'"), "{text}");
    assert!(!text.contains("SECRET SKILL BODY"), "{text}");
}

#[tokio::test]
async fn dispatch_skills_remove_project_skill_points_to_disable() {
    let _output_guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    let mut s = skill("project-skill", "SECRET SKILL BODY", false);
    s.source = SkillSource::Project;
    s.file_path = temp
        .path()
        .join(".theway")
        .join("skills")
        .join("project-skill")
        .join("SKILL.md")
        .to_string_lossy()
        .to_string();
    opts.skills = vec![s];
    let harness = Arc::new(AgentHarness::new(opts));
    let _executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
    };

    let outcome = commands::dispatch("/skills remove project-skill", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(msg) => {
            assert!(msg.contains("cannot be removed"), "{msg}");
            assert!(msg.contains("/skills disable project-skill"), "{msg}");
            assert!(!msg.contains("SECRET SKILL BODY"), "{msg}");
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}
