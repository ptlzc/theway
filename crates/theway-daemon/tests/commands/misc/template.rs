//! `/template` — empty catalog, loaded catalog, name+var dispatch, and arg errors.

use super::*;
use theway_core::{AgentHarness, AgentHarnessOptions, PromptTemplate};
use theway_transport::commands::CommandOutcome;

// ───────────────────────────────────────────────────────────────────────────────────────
// /template
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn template_lists_empty_catalog_message() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("(no templates loaded"), "{text}");
}

#[tokio::test]
async fn template_lists_loaded_templates() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let options = AgentHarnessOptions {
        prompt_templates: vec![
            PromptTemplate {
                name: "greet".into(),
                description: Some("say hello".into()),
                content: "hello {{who}}".into(),
                file_path: "/tmp/greet.md".into(),
            },
            PromptTemplate {
                name: "plain".into(),
                description: None,
                content: "plain body".into(),
                file_path: "/tmp/plain.md".into(),
            },
        ],
        ..AgentHarnessOptions::new(faux_model(), session)
    };
    let harness = Arc::new(AgentHarness::new(options));
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("Loaded templates (2):"), "{text}");
    assert!(text.contains("/template greet  say hello"), "{text}");
    assert!(text.contains("/template plain  "), "{text}");
}

#[tokio::test]
async fn template_with_name_returns_run_prompt_template() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand
        .run(&["greet".into(), "who=world".into(), "x=1".into()], &ctx)
        .await;

    match outcome {
        CommandOutcome::RunPromptTemplate { name, vars } => {
            assert_eq!(name, "greet");
            assert_eq!(vars.get("who").and_then(|v| v.as_str()), Some("world"));
            assert_eq!(vars.get("x").and_then(|v| v.as_str()), Some("1"));
        }
        other => panic!("expected RunPromptTemplate, got {other:?}"),
    }
}

#[tokio::test]
async fn template_rejects_arg_without_equals() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand
        .run(&["greet".into(), "badarg".into()], &ctx)
        .await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("expected k=v argument; got: badarg"))
    );
}
