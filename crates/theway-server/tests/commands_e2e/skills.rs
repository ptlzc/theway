//! Skill command suites: dynamic skill slash shortcuts, `/help` listing, `/skill`,
//! and `/skills` show/reload/enable/disable/install/remove with overlay persistence.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
    SessionTreeEntry, SkillSource,
};

use super::helpers::*;
use crate::auth;
use crate::commands;
use crate::skill_overrides;

#[tokio::test]
async fn dynamic_skill_slash_command_attaches_skill_without_body_echo() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let _capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![skill("db9", "SECRET SKILL BODY", false)];
    let harness = Arc::new(AgentHarness::new(opts));
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
    };
    let outcome = commands::dispatch("/db9", &registry, &ctx).await;

    match outcome {
        commands::CommandOutcome::AttachSkill { name } => assert_eq!(name, "db9"),
        other => panic!("expected AttachSkill outcome, got {other:?}"),
    }
    let output = _capture.text();
    assert!(output.contains("using skill: db9 (user)"), "{output}");
    assert!(!output.contains("SECRET SKILL BODY"), "{output}");
}

#[tokio::test]
async fn dynamic_skill_slash_command_with_prompt_runs_skill_wrapped_turn() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let _capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![skill("db9", "SECRET SKILL BODY", false)];
    let harness = Arc::new(AgentHarness::new(opts));
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
    };
    let outcome = commands::dispatch("/db9 create a table", &registry, &ctx).await;

    match outcome {
        commands::CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert!(prompt.contains("Skill tool"));
            assert!(prompt.contains("db9"));
            assert!(prompt.contains("create a table"));
            assert!(!prompt.contains("SECRET SKILL BODY"));
        }
        other => panic!("expected RunAgentPrompt outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn dynamic_skill_slash_command_hides_disabled_and_builtin_conflicts() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![
        skill("disabled-skill", "body", true),
        skill("help", "conflicting body", false),
    ];
    let harness = Arc::new(AgentHarness::new(opts));
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
    let shortcuts = commands::skill_shortcuts(&harness.skills(), &registry);
    assert!(
        shortcuts
            .iter()
            .all(|shortcut| shortcut.command != "/disabled-skill")
    );
    assert!(shortcuts.iter().all(|shortcut| shortcut.command != "/help"));

    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };
    let outcome = commands::dispatch("/disabled-skill", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(msg) => {
            assert!(msg.contains("/skills enable"), "{msg}");
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn help_lists_dynamic_skill_commands_without_body() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![
        skill("db9", "SECRET SKILL BODY", false),
        skill("hidden-skill", "SECRET HIDDEN BODY", true),
    ];
    let harness = Arc::new(AgentHarness::new(opts));
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
    };
    let outcome = commands::dispatch("/help", &registry, &ctx).await;

    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    let text = capture.text();
    assert!(text.contains("Skill commands:"), "{text}");
    assert!(text.contains("/db9 [prompt]"), "{text}");
    assert!(!text.contains("/hidden-skill"), "{text}");
    assert!(!text.contains("SECRET"), "{text}");
}

#[tokio::test]
async fn dispatch_skill_attaches_loaded_skill_without_exposing_body() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![skill("review-pr", "SECRET SKILL BODY", false)];
    let harness = Arc::new(AgentHarness::new(opts));
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
    };

    let outcome = commands::dispatch("/skill review-pr", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::AttachSkill { name } => assert_eq!(name, "review-pr"),
        other => panic!("expected AttachSkill outcome, got {other:?}"),
    }

    let prompt = commands::attach_skill_prompt("summarize the diff", Some("review-pr"));
    assert!(prompt.contains("Skill tool"));
    assert!(prompt.contains("review-pr"));
    assert!(prompt.contains("summarize the diff"));
    assert!(
        !prompt.contains("SECRET SKILL BODY"),
        "slash command must not inline skill body into the user-visible prompt"
    );
}

#[tokio::test]
async fn dispatch_skill_refuses_disabled_skill() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![skill("disabled-skill", "SECRET SKILL BODY", true)];
    let harness = Arc::new(AgentHarness::new(opts));
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
    };

    let outcome = commands::dispatch("/skill disabled-skill", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(msg) => {
            assert!(msg.contains("disabled-skill"));
            assert!(msg.contains("disable_model_invocation=true"));
            assert!(!msg.contains("SECRET SKILL BODY"));
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}

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

#[tokio::test]
async fn dispatch_skill_unknown_name_suggests_prefix_matches() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = vec![skill("review-pr", "SECRET SKILL BODY", false)];
    let harness = Arc::new(AgentHarness::new(opts));
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
    };

    let outcome = commands::dispatch("/skill rev", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(msg) => {
            assert!(msg.contains("no skill named 'rev'"));
            assert!(msg.contains("Did you mean: review-pr"));
            assert!(!msg.contains("SECRET SKILL BODY"));
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}
