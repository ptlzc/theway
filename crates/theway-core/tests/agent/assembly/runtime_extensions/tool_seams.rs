//! Tool seams: `transform_tool_result`, tool-execution observation, and
//! `before_tool_call` blocked mapping.

use super::*;

// ── tool seams ─────────────────────────────────────────────────────────────────────────

fn coverage_tool_call() -> ToolCall {
    ToolCall {
        id: "tool-1".into(),
        name: "coverage-tool".into(),
        arguments: serde_json::Map::new(),
        thought_signature: None,
    }
}

fn coverage_after_tool_context() -> AfterToolCallContext {
    AfterToolCallContext {
        assistant_message: assistant("assistant"),
        tool_call: coverage_tool_call(),
        args: serde_json::json!({}),
        result: AgentToolResult::default(),
        is_error: false,
        context: AgentContext::default(),
    }
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_invocation_error_returns_default() {
    let runtime = coverage_runtime_with_cwd("");
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_dispatch_error_returns_default() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult
            && invocation.class() == ExtensionHookClass::Transform
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "tool result failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_invalid_follow_up_returns_default() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult
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
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_enqueue_follow_up_capacity_error_returns_default() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult
            && invocation.class() == ExtensionHookClass::Transform
        {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("tool-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_observe_tool_execution_ignores_invocation_error_with_empty_cwd() {
    let runtime = Arc::new(coverage_runtime_with_cwd(""));
    let (listener, _) = runtime.make_loop_listener();
    listener(
        crate::types::LoopEvent::ToolExecutionStart {
            tool_call_id: "tool-1".into(),
            tool_name: "coverage-tool".into(),
            args: serde_json::json!({}),
        },
        CancellationToken::new(),
    )
    .await;
}

#[tokio::test]
async fn runtime_ext_before_tool_call_blocked_error_non_cancelled() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolCall
            && invocation.class() == ExtensionHookClass::Gate
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "limit reached"))
        } else {
            Ok(empty_batch())
        }
    });
    let context = crate::types::BeforeToolCallContext {
        assistant_message: assistant("assistant"),
        tool_call: coverage_tool_call(),
        args: serde_json::json!({}),
        context: AgentContext::default(),
    };
    let result = runtime.before_tool_call(&context, &CancellationToken::new()).await;
    assert!(result.block);
    assert!(result.reason.unwrap().contains("resource_limit"));
}

#[tokio::test]
async fn runtime_ext_before_tool_call_blocked_error_malformed_cancelled() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolCall
            && invocation.class() == ExtensionHookClass::Gate
        {
            Err(coverage_error(ExtensionErrorCode::Cancelled, "no separator here"))
        } else {
            Ok(empty_batch())
        }
    });
    let context = crate::types::BeforeToolCallContext {
        assistant_message: assistant("assistant"),
        tool_call: coverage_tool_call(),
        args: serde_json::json!({}),
        context: AgentContext::default(),
    };
    let result = runtime.before_tool_call(&context, &CancellationToken::new()).await;
    assert!(result.block);
    assert!(result.reason.unwrap().contains("cancelled"));
}
