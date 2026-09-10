//! `/skills enable`, `/skills disable`, `/skills show`, and `/skills reload`
//! with overlay persistence and session audit entries.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
    SessionTreeEntry, SkillSource,
};

use super::super::helpers::*;
use crate::auth;
use crate::commands;
use crate::skill_overrides;

#[tokio::test]
async fn dispatch_skills_disable_persists_overlay_and_reloads() {
    let _auth_guard = auth::ENV_LOCK.lock().unwrap();
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let harness = harness_with_reloadable_skills(
        temp.path(),
        vec![skill("review-pr", "SECRET SKILL BODY", false)],
    );
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

    let outcome = commands::dispatch("/skills disable review-pr", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let skills = harness.skills();
    let skill = skills.iter().find(|s| s.name == "review-pr").unwrap();
    assert!(
        skill.disable_model_invocation,
        "reload should apply overlay"
    );

    let state = skill_overrides::load(temp.path()).await;
    assert_eq!(
        state
            .lookup("review-pr", SkillSource::User)
            .map(|entry| entry.enabled),
        Some(false)
    );
    let entries = harness.session().entries().await.unwrap();
    let audit = entries.iter().any(|entry| {
        matches!(
            entry,
            SessionTreeEntry::Custom { custom_type, data, .. }
                if custom_type == "skill_control_plane"
                    && data.as_ref().and_then(|d| d.get("actor")).and_then(|v| v.as_str()) == Some("slash")
                    && data.as_ref().and_then(|d| d.get("after_enabled")).and_then(|v| v.as_bool()) == Some(false)
        )
    });
    assert!(
        audit,
        "slash skill disable should write audit: {entries:#?}"
    );
}

#[tokio::test]
async fn dispatch_skills_enable_is_user_mediated_and_reuses_overlay() {
    let _auth_guard = auth::ENV_LOCK.lock().unwrap();
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let harness = harness_with_reloadable_skills(
        temp.path(),
        vec![skill("formatter", "SECRET SKILL BODY", true)],
    );
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

    let outcome = commands::dispatch("/skills enable formatter user", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let skills = harness.skills();
    let skill = skills.iter().find(|s| s.name == "formatter").unwrap();
    assert!(
        !skill.disable_model_invocation,
        "user slash command may explicitly enable a frontmatter-disabled skill"
    );

    let state = skill_overrides::load(temp.path()).await;
    assert_eq!(
        state
            .lookup("formatter", SkillSource::User)
            .map(|entry| entry.enabled),
        Some(true)
    );
    let entries = harness.session().entries().await.unwrap();
    let audit = entries.iter().any(|entry| {
        matches!(
            entry,
            SessionTreeEntry::Custom { custom_type, data, .. }
                if custom_type == "skill_control_plane"
                    && data.as_ref().and_then(|d| d.get("actor")).and_then(|v| v.as_str()) == Some("slash")
                    && data.as_ref().and_then(|d| d.get("after_enabled")).and_then(|v| v.as_bool()) == Some(true)
        )
    });
    assert!(audit, "slash skill enable should write audit: {entries:#?}");
}

#[tokio::test]
async fn dispatch_skills_show_prints_metadata_without_body() {
    let _output_guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    let mut s = skill("review-pr", "SECRET SKILL BODY", false);
    s.source = SkillSource::Project;
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

    let outcome = commands::dispatch("/skills show review-pr project", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    let text = capture.text();
    assert!(text.contains("Skill: review-pr (project)"), "{text}");
    assert!(text.contains("Status: enabled"), "{text}");
    assert!(text.contains("Path:"), "{text}");
    assert!(
        text.contains("Body: not shown"),
        "show should explain body omission:\n{text}"
    );
    assert!(
        !text.contains("SECRET SKILL BODY"),
        "show must not print SKILL.md body:\n{text}"
    );
}

#[tokio::test]
async fn dispatch_skills_reload_uses_harness_reload_and_prints_summary() {
    let _output_guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let capture = OutputCapture::install();
    let temp = tempfile::tempdir().unwrap();
    let harness = harness_with_reloadable_skills(
        temp.path(),
        vec![skill("one", "body", false), skill("two", "body", false)],
    );
    // Make the live catalog stale so the assertion proves `/skills reload` called the harness
    // reload closure rather than just recounting the current catalog.
    harness.replace_skills(Vec::new());
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

    let outcome = commands::dispatch("/skills reload", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert_eq!(harness.skills().len(), 2, "reload should refresh catalog");
    let text = capture.text();
    assert!(
        text.contains("reloaded skills: 2 loaded, 0 diagnostics"),
        "{text}"
    );
}
