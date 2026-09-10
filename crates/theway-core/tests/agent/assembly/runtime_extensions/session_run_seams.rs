//! Session and run seam fallbacks in `HarnessRuntimeExtensions`: session
//! start/shutdown idempotency, model selection, observation, and run gates.

use super::*;

// ── lifecycle idempotency + invocation error paths ─────────────────────────────────────

#[tokio::test]
async fn runtime_ext_ensure_session_start_second_call_is_noop() {
    let runtime = coverage_runtime_with_cwd("/workspace");
    runtime.ensure_session_start().await;
    runtime.ensure_session_start().await;
}

#[tokio::test]
async fn runtime_ext_ensure_session_start_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_runtime_with_cwd("");
    runtime.ensure_session_start().await;
}

#[tokio::test]
async fn runtime_ext_shutdown_second_call_is_noop() {
    let runtime = coverage_runtime_with_cwd("/workspace");
    runtime.shutdown().await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_ext_shutdown_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_runtime_with_cwd("");
    runtime.shutdown().await;
}

// ── remaining invocation-error paths ───────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_before_model_selection_invocation_error_is_reported() {
    let runtime = coverage_runtime_with_cwd("");
    assert!(runtime.before_model_selection(&faux_model()).await.is_err());
}

#[tokio::test]
async fn runtime_ext_model_selected_handles_success_and_invocation_error() {
    let runtime = coverage_runtime_with_cwd("/workspace");
    runtime.model_selected(&faux_model()).await;

    let runtime = coverage_runtime_with_cwd("");
    runtime.model_selected(&faux_model()).await;
}

#[tokio::test]
async fn runtime_ext_observe_session_operation_ignores_invocation_error() {
    let runtime = coverage_runtime_with_cwd("");
    runtime
        .observe_session_operation(ExtensionLifecycleEvent::SessionStart, serde_json::json!({}))
        .await;
}

#[tokio::test]
async fn runtime_ext_observe_run_ignores_invocation_error() {
    let runtime = coverage_runtime_with_cwd("");
    runtime
        .observe_run(ExtensionLifecycleEvent::RunStarted, serde_json::json!({}), false)
        .await;
}

#[tokio::test]
async fn runtime_ext_before_run_invocation_error_returns_default_patch() {
    let runtime = coverage_runtime_with_cwd("");
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_dispatch_error_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "before-run failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_patch_deserialize_error_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::PatchRunContext,
                    payload: serde_json::json!({"systemPrompt": 42}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_invalid_follow_up_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_enqueue_follow_up_capacity_error_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("before-run-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_gate_allows_enqueues_follow_up_and_allows() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeSessionSwitch
            && invocation.class() == ExtensionHookClass::Gate
        {
            Ok(ExtensionActionBatch {
                decision: Some(ExtensionGateDecision::Allow),
                actions: vec![coverage_follow_up_action("gate-follow-up", user_message("follow"))],
            })
        } else {
            Ok(empty_batch())
        }
    });
    runtime
        .gate_session_operation(ExtensionLifecycleEvent::BeforeSessionSwitch, serde_json::json!({}))
        .await
        .unwrap();
    assert!(runtime.take_follow_up().is_some());
}
