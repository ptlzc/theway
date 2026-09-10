//! Dynamic skill slash shortcuts, `/help` listing, and `/skill` dispatch.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};

use super::super::helpers::*;
use crate::commands;

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
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
    };
    let outcome = commands::dispatch("/db9", &registry, &ctx).await;

    match outcome {
        commands::CommandOutcome::AttachSkill { name } => assert_eq!(name, "db9"),
        other => panic!("expected AttachSkill outcome, got {other:?}"),
    }
    let output = _capture.text();
    assert!(
        !output.contains("using skill"),
        "skill shortcut must not print a duplicate using-skill line: {output}"
    );
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
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
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
    // /help is TUI-local now (daemon-kernel-layers); the daemon registry
    // must not expose it, so no skill shortcut may collide with it either way.
    assert!(registry.find("help").is_none());

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
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
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
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
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
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
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
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
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
