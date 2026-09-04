//! Control-plane prompt hook suite: `/new-trigger` routes through the agent with a
//! permission-gated `NewTriggerTool`; without an explicit `on_control_plane_prompt`
//! hook the harness defaults to fail-closed deny, so these tests install the
//! auto-approve hook and verify the post-approval registration path.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentTool, MemorySessionStorage, Session, SessionStorage,
};

use super::helpers::*;
use crate::commands;
use crate::triggers;

#[tokio::test(flavor = "current_thread")]
async fn dispatch_new_trigger_registers_dynamic_rule() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.tools = vec![Arc::new(triggers::NewTriggerTool::new(
        triggers::global_registry().clone(),
    )) as Arc<dyn AgentTool>];
    opts.stream_fn = Some(new_trigger_extraction_stream());
    // Issue #110 sub-PR 3: NewTriggerTool::permission_classification returns
    // Prompt — without an explicit hook the harness defaults to fail-closed
    // deny and the tool never runs. This test focuses on the post-approval
    // path, so install an auto-approve hook. Production deployments rely on
    // the embedder's real prompt card; see PR #138 for the CLI/TUI wiring.
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
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
    };

    let condition = "\u{73b0}\u{5728}\u{662f} 11pm";
    let action = "\u{5199}\u{4e00}\u{4e2a} tmp \u{6587}\u{4ef6}";
    let prompt =
        format!("/new-trigger \u{968f}\u{4fbf}\u{8bf4}\u{4e00}\u{53e5}: {condition}; {action}");

    let outcome = commands::dispatch(&prompt, &registry, &ctx).await;
    let agent_prompt = match outcome {
        commands::CommandOutcome::RunAgentPrompt {
            prompt,
            error_context,
        } => {
            assert_eq!(error_context, "create trigger: ");
            assert!(prompt.contains(condition));
            assert!(prompt.contains(action));
            prompt
        }
        other => panic!("expected RunAgentPrompt outcome, got {other:?}"),
    };
    assert!(
        triggers::global_registry().list().is_empty(),
        "/new-trigger dispatch should not run the agent directly; the TUI owns Ctrl-C abort handling"
    );

    harness.prompt(agent_prompt).await.unwrap();

    let rules = triggers::global_registry().list();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].condition, condition);
    assert_eq!(rules[0].action, action);
    let status_lines = commands::render_triggers_status(
        &executor.notification_status_snapshot(),
        triggers::global_registry(),
    );
    assert!(
        status_lines
            .iter()
            .any(|line| line.contains("dynamic rules: 1"))
    );
    assert!(status_lines.iter().any(|line| line.contains(&rules[0].id)));
    assert!(status_lines.iter().any(|line| line.contains("tmp")));
    assert!(
        !session.entries().await.unwrap().is_empty(),
        "/new-trigger routes through the agent so the model can extract condition/action"
    );
}
