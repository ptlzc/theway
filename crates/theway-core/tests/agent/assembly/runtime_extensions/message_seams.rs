//! Message seams: `observe_message_update` and `transform_message` error,
//! payload, and follow-up limit paths.

use super::*;

// ── message seams ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_observe_message_update_without_message_id_is_noop() {
    let runtime = Arc::new(coverage_runtime_with_cwd("/workspace"));
    let (listener, _) = runtime.make_loop_listener();
    listener(
        crate::types::LoopEvent::MessageUpdate {
            message: user_message("hello"),
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(""),
            },
        },
        CancellationToken::new(),
    )
    .await;
}

#[tokio::test]
async fn runtime_ext_transform_message_invocation_error_returns_message() {
    let runtime = coverage_runtime_with_cwd("");
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_dispatch_error_returns_message() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "message failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_replace_payload_missing_message_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceMessage,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_replace_invalid_json_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceMessage,
                    payload: serde_json::json!({"message": 42}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_role_mismatch_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceMessage,
                    payload: serde_json::json!({"message": coverage_assistant_message("assistant")}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_invalid_follow_up_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
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
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_enqueue_follow_up_capacity_error_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("message-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}
