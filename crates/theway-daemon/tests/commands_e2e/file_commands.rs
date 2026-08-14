//! Dispatch-level tests for the claude-code-format file commands and the
//! `/reload` rescan (issue #37).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use theway_core::{
    AgentHarness, AgentHarnessOptions, LoadSkillsOutput, MemorySessionStorage, ReloadSkillsFn,
    Session, SessionStorage,
};

use super::helpers::*;
use crate::commands;

/// Harness + executor + registry + a `CommandCtx` rooted at `cwd` (tempdir).
fn harness_and_registry() -> (
    Arc<AgentHarness>,
    Arc<crate::trigger_engine::execution::TriggerExecutor>,
    commands::Registry,
) {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
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
    (harness, executor, commands::Registry::with_builtins())
}

#[tokio::test]
async fn dispatch_file_command_expands_arguments_into_prompt() {
    let (harness, executor, registry) = harness_and_registry();
    let cwd = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let commands_dir = cwd.path().join(".claude/commands");
    std::fs::create_dir_all(&commands_dir).unwrap();
    std::fs::write(
        commands_dir.join("commit.md"),
        "---\ndescription: write a commit message\n---\ncommit message for $1\nrest: $ARGUMENTS\n",
    )
    .unwrap();
    registry.set_file_commands(crate::file_commands::scan_file_commands_in(
        cwd.path(),
        user.path(),
    ));
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: cwd.path(),
    };

    let outcome = commands::dispatch("/commit the whole diff", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert_eq!(prompt, "commit message for the\nrest: the whole diff");
        }
        other => panic!("expected RunAgentPrompt outcome, got {other:?}"),
    }

    // Without arguments the placeholders stay literal.
    let bare = commands::dispatch("/commit", &registry, &ctx).await;
    match bare {
        commands::CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert_eq!(prompt, "commit message for $1\nrest: ");
        }
        other => panic!("expected RunAgentPrompt outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_reload_rescans_skills_and_file_commands() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let reloads = Arc::new(AtomicUsize::new(0));
    let reload_counter = reloads.clone();
    let loader: ReloadSkillsFn = Arc::new(move || {
        let counter = reload_counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            LoadSkillsOutput {
                skills: Vec::new(),
                diagnostics: Vec::new(),
            }
        })
    });
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.reload_skills_fn = Some(loader);
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
    let cwd = tempfile::tempdir().unwrap();
    let _user = tempfile::tempdir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: cwd.path(),
    };

    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let _capture = OutputCapture::install();
    let outcome = commands::dispatch("/reload", &registry, &ctx).await;
    assert!(
        matches!(outcome, commands::CommandOutcome::Handled),
        "reload should succeed: {outcome:?}"
    );
    assert_eq!(reloads.load(Ordering::SeqCst), 1, "skills must be reloaded");

    // A command file created after startup shows up after /reload. (The scan
    // also reads the real user roots, so assert membership, not equality.)
    let commands_dir = cwd.path().join(".agents/commands");
    std::fs::create_dir_all(&commands_dir).unwrap();
    std::fs::write(commands_dir.join("deploy.md"), "deploy $ARGUMENTS\n").unwrap();
    let _ = commands::dispatch("/reload", &registry, &ctx).await;
    let names = registry.file_command_names();
    assert!(
        names.contains(&"/deploy".to_string()),
        "file commands must be rescanned: {names:?}"
    );
    assert_eq!(reloads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn dispatch_plain_message_does_not_error_for_path_like_slash() {
    let (harness, executor, registry) = harness_and_registry();
    let cwd = tempfile::tempdir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: cwd.path(),
    };
    let outcome = commands::dispatch("/etc/hosts", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert_eq!(prompt, "/etc/hosts");
        }
        other => panic!("expected RunAgentPrompt outcome, got {other:?}"),
    }
}
