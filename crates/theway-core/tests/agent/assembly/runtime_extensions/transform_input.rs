//! `transform_input` fallbacks: invocation and dispatch errors, payload
//! validation, and follow-up enqueue limits.

use super::*;

// ── transform_input fallbacks ──────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_transform_input_invocation_error_returns_original() {
    let runtime = coverage_runtime_with_cwd("");
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_dispatch_error_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "input failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_replace_payload_missing_message_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceInput,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_replace_invalid_json_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceInput,
                    payload: serde_json::json!({"message": 42}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_replace_role_mismatch_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceInput,
                    payload: serde_json::json!({"message": coverage_assistant_message("assistant")}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_emit_command_outcome_invalid_payload_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EmitCommandOutcome,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_enqueue_follow_up_valid_is_accepted() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![coverage_follow_up_action("input-follow-up", user_message("follow"))],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_enqueue_follow_up_invalid_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
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
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_enqueue_follow_up_capacity_error_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("input-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

// parse_follow_up branches exercised through transform_input

#[tokio::test]
async fn runtime_ext_transform_input_rejects_empty_follow_up_id() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": "",
                        "message": user_message("follow"),
                    }),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_rejects_oversized_follow_up_id() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": "x".repeat(129),
                        "message": user_message("follow"),
                    }),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_rejects_non_user_follow_up_message() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": "valid-id",
                        "message": coverage_assistant_message("assistant"),
                    }),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}
