//! `transform_context` fallbacks and follow-up dedup capacity.

use super::*;

// ── transform_context fallbacks ────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_transform_context_invocation_error_returns_messages() {
    let runtime = coverage_runtime_with_cwd("");
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

#[tokio::test]
async fn runtime_ext_transform_context_dispatch_error_returns_messages() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Context {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "context failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

#[tokio::test]
async fn runtime_ext_transform_context_invalid_follow_up_returns_messages() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Context {
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
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

#[tokio::test]
async fn runtime_ext_transform_context_enqueue_follow_up_capacity_error_returns_messages() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Context {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("context-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

// Dedup-eviction loop: after 8 full batches (256 unique ids) the 257th unique id
// triggers `while seen_order.len() > FOLLOW_UP_DEDUP_CAPACITY` and `pop_front()`.

#[tokio::test]
async fn runtime_ext_follow_up_dedup_evicts_oldest_seen_id_after_capacity() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = coverage_runtime_with_handler("/workspace", {
        let calls = Arc::clone(&calls);
        move |invocation| {
            if invocation.event() == ExtensionLifecycleEvent::Context {
                let batch_index = calls.fetch_add(1, Ordering::Relaxed);
                let actions = (0..32)
                    .map(|index| {
                        coverage_follow_up_action(
                            &format!("dedup-{}", batch_index * 32 + index),
                            user_message("follow"),
                        )
                    })
                    .collect();
                Ok(ExtensionActionBatch {
                    decision: None,
                    actions,
                })
            } else {
                Ok(empty_batch())
            }
        }
    });

    for _ in 0..9 {
        runtime
            .transform_context(vec![user_message("a")], CancellationToken::new())
            .await;
        while runtime.take_follow_up().is_some() {}
    }
}
