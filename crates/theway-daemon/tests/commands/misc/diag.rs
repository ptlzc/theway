//! `/diag` — the diagnostic snapshot line.

use super::*;
use theway_core::ThinkingLevel;
use theway_transport::commands::{CommandCtx, CommandOutcome};

// ───────────────────────────────────────────────────────────────────────────────────────
// /diag
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn diag_prints_model_thinking_tools_skills_cost_and_log() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let harness = harness_with(session);
    harness.agent().state().thinking_level = Some(ThinkingLevel::High);

    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("session.log");
    let ctx = CommandCtx {
        session_id: "sess-1",
        log_path: Some(&log_path),
        tool_count: 3,
        cwd: tmp.path(),
        extra: &extra,
    };

    let outcome = DiagCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let lines = capture.lines();
    let text = lines.join("\n");
    assert!(text.contains("Diagnostic snapshot:"), "{text}");
    assert!(text.contains("session       sess-1"), "{text}");
    assert!(text.contains("model         faux:faux"), "{text}");
    assert!(text.contains("thinking      high"), "{text}");
    assert!(text.contains("tools         3"), "{text}");
    assert!(text.contains("skills        0"), "{text}");
    assert!(text.contains("cost"), "{text}");
    assert!(text.contains(&log_path.display().to_string()), "{text}");
}

#[tokio::test]
async fn diag_handles_missing_model_and_thinking() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let harness = harness_with(session);
    {
        let mut state = harness.agent().state();
        state.model = None;
        state.thinking_level = None;
    }

    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = DiagCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("model         (none)"), "{text}");
    assert!(text.contains("thinking      ?"), "{text}");
    assert!(text.contains("log file      (logging disabled)"), "{text}");
}
