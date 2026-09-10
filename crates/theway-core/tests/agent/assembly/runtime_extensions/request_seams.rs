//! Request seams: `before_model_request` error, payload, and follow-up
//! limit paths.

use super::*;

// ── request seams ──────────────────────────────────────────────────────────────────────

fn coverage_request_draft() -> NormalizedModelRequestDraft {
    NormalizedModelRequestDraft {
        provider: "test-provider".into(),
        model: "test-model".into(),
        system_instructions: Some("base system".into()),
        messages: Vec::new(),
        visible_tools: vec![
            theway_llm_provider::Tool {
                name: "bash".into(),
                description: "bash tool".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
            theway_llm_provider::Tool {
                name: "edit".into(),
                description: "edit tool".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ],
        executable_tool_names: vec!["bash".into(), "edit".into()],
        generation_options: NormalizedGenerationOptions::default(),
    }
}

fn coverage_assert_request_unchanged(
    accepted: NormalizedModelRequestDraft,
    base: &NormalizedModelRequestDraft,
) {
    assert_eq!(
        serde_json::to_value(accepted).unwrap(),
        serde_json::to_value(base).unwrap()
    );
}

#[tokio::test]
async fn runtime_ext_before_model_request_invocation_error_returns_request() {
    let runtime = coverage_runtime_with_cwd("");
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_dispatch_error_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "request failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_replace_payload_invalid_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceModelRequest,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_invalid_follow_up_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
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
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_enqueue_follow_up_capacity_error_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("request-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}
