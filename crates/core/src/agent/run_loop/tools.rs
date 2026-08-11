//! Tool-call preparation (permission gates, hooks) and execution for the agent loop.

use std::sync::Arc;

use theway_llm_provider::{
    AssistantMessage as PiAssistantMessage, ToolResultMessage, UserContentBlock,
};
use tokio_util::sync::CancellationToken;

use crate::agent::AgentInner;
use crate::types::*;

use super::run_one;
use super::utils::{compute_args_hash, default_prompt_payload, emit, snapshot_context};

/// Execute every tool-call block in the assistant's content. Returns the per-call results
/// (in assistant content order) and `all_terminate = true` when every result hints early
/// termination.
pub(super) async fn execute_tools(
    inner: &Arc<AgentInner>,
    assistant: &PiAssistantMessage,
    cancel: &CancellationToken,
) -> (Vec<ToolResultMessage>, bool) {
    // Gather the tool calls + matched AgentTool implementations in assistant content order.
    let tool_calls: Vec<&theway_llm_provider::ToolCall> = assistant
        .content
        .iter()
        .filter_map(|b| match b {
            theway_llm_provider::ContentBlock::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .collect();
    if tool_calls.is_empty() {
        return (Vec::new(), false);
    }
    let tools_snapshot = inner.state.lock().tools.clone();

    // Decide per-call execution mode (parallel default unless any tool requests sequential).
    let mode = inner.options.tool_execution;
    let any_sequential = tool_calls.iter().any(|tc| {
        let matched = tools_snapshot
            .iter()
            .find(|t| t.definition().name == tc.name);
        matched
            .and_then(|t| t.execution_mode())
            .map(|m| matches!(m, ToolExecutionMode::Sequential))
            .unwrap_or(false)
    });
    let mode = if any_sequential {
        ToolExecutionMode::Sequential
    } else {
        mode
    };

    // Pre-flight: run `before_tool_call` for every call. If a hook blocks, synthesize an error
    // result and skip the actual execute. Returns Vec<Option<execute_input>> in call order.
    let mut prepared: Vec<PreparedCall> = Vec::with_capacity(tool_calls.len());
    let agent_context = snapshot_context(inner);
    for tc in &tool_calls {
        let tool_id = tc.id.clone();
        let tool_name = tc.name.clone();
        let raw_args = serde_json::Value::Object(tc.arguments.clone());

        // Resolve the tool BEFORE normalizing args so we can run its `prepare_arguments`
        // compatibility shim. Unknown tools keep raw args (the dispatcher will produce a
        // "no such tool" error result downstream).
        let tool = tools_snapshot
            .iter()
            .find(|t| t.definition().name == tool_name)
            .cloned();
        let args = match &tool {
            Some(t) => t.prepare_arguments(raw_args),
            None => raw_args,
        };

        emit(
            inner,
            AgentEvent::ToolExecutionStart {
                tool_call_id: tool_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            },
            cancel,
        )
        .await;

        // Per-tool classification runs first (issue #110 design v0.2 Artifact A). The
        // classifier sees the prepared args and decides Allow / Prompt / Block before the
        // user-configured `before_tool_call` hook gets a chance. `Block` short-circuits
        // immediately (no `before_tool_call`, no prompt); `Prompt` synthesizes a default
        // `BeforeToolCallResult::prompt` that the user hook can override; `Allow` falls
        // through to the existing `before_tool_call` path with no synthesized prompt.
        let classification = match &tool {
            Some(t) => t.permission_classification(&args),
            None => PermissionClassification::Allow,
        };
        if let PermissionClassification::Block { reason } = &classification {
            let result = AgentToolResult {
                content: vec![UserContentBlock::text(reason.clone())],
                details: serde_json::Value::Null,
                terminate: None,
            };
            prepared.push(PreparedCall::Blocked {
                id: tool_id,
                name: tool_name,
                args,
                result,
            });
            continue;
        }

        // The classifier's `Prompt` is the authoritative source: a user-configured
        // `before_tool_call` hook MUST NOT silently erase a control-plane prompt requirement
        // by returning `BeforeToolCallResult::default()`. We preserve the synthesized prompt
        // unless the hook either explicitly hard-blocks (`block=true` wins, classifier
        // intent honored — Block-stronger-than-Prompt) or supplies its own richer
        // `BeforeToolCallResult::prompt` payload (which the runtime then re-binds to the
        // authoritative `tool_call_id` / `tool_name` / `args_hash` below — the hook may
        // only enrich `label` and `payload`, never spoof binding fields).
        //
        // The hook still sees the prepared args on BOTH `ctx.args` and
        // `ctx.tool_call.arguments` (matched semantics from the legacy code path). If the
        // tool's `prepare_arguments` returns a non-Object shape we clear the map so the
        // hook author has only one truthy source.
        let synthesized_prompt: Option<ControlPlanePromptRequest> = match &classification {
            PermissionClassification::Prompt { reason } => Some(ControlPlanePromptRequest {
                tool_call_id: tool_id.clone(),
                tool_name: tool_name.clone(),
                args_hash: compute_args_hash(&args),
                label: format!("Control-plane write: {tool_name}"),
                payload: default_prompt_payload(&tool_name, &args),
                reason: reason.clone(),
            }),
            PermissionClassification::Allow => None,
            // Block already handled by the early-return above; kept for exhaustiveness.
            PermissionClassification::Block { .. } => unreachable!(),
        };

        let mut hook_result = BeforeToolCallResult {
            block: false,
            reason: None,
            prompt: synthesized_prompt.clone(),
        };
        if let Some(hook) = inner.options.before_tool_call.clone() {
            let mut hook_tc = (*tc).clone();
            hook_tc.arguments = match &args {
                serde_json::Value::Object(map) => map.clone(),
                _ => serde_json::Map::new(),
            };
            let ctx = BeforeToolCallContext {
                assistant_message: assistant.clone(),
                tool_call: hook_tc,
                args: args.clone(),
                context: agent_context.clone(),
            };
            hook_result = hook(ctx, cancel.clone()).await;
        }
        if hook_result.block {
            let reason = hook_result
                .reason
                .unwrap_or_else(|| "tool call blocked by before_tool_call hook".to_string());
            let result = AgentToolResult {
                content: vec![UserContentBlock::text(reason)],
                details: serde_json::Value::Null,
                terminate: None,
            };
            prepared.push(PreparedCall::Blocked {
                id: tool_id,
                name: tool_name,
                args,
                result,
            });
            continue;
        }
        // Merge: if the classifier requested a Prompt, ensure the runtime still routes
        // through the prompt channel even if the hook returned `prompt = None`. If the hook
        // supplied its own prompt, accept it as the embedder's richer card BUT re-bind
        // `tool_call_id` / `tool_name` / `args_hash` to the runtime-authoritative values so
        // a hook cannot lie about binding fields (forgery resistance).
        let effective_prompt: Option<ControlPlanePromptRequest> =
            match (synthesized_prompt, hook_result.prompt.take()) {
                // Classifier said Prompt, hook didn't supply one → keep the classifier's.
                (Some(synth), None) => Some(synth),
                // Classifier said Allow but hook supplied a prompt → accept it (hook is
                // raising the bar). Runtime still owns binding fields.
                (None, Some(hook_supplied)) => Some(ControlPlanePromptRequest {
                    tool_call_id: tool_id.clone(),
                    tool_name: tool_name.clone(),
                    args_hash: compute_args_hash(&args),
                    label: hook_supplied.label,
                    payload: hook_supplied.payload,
                    reason: hook_supplied.reason,
                }),
                // Classifier said Prompt AND hook supplied a custom payload → use hook's
                // label/payload (richer card) BUT re-bind authoritative fields. Hook cannot
                // override the classifier's `reason` (it's the reason the gate exists), but
                // can supply additional context via `payload`.
                (Some(synth), Some(hook_supplied)) => Some(ControlPlanePromptRequest {
                    tool_call_id: synth.tool_call_id,
                    tool_name: synth.tool_name,
                    args_hash: synth.args_hash,
                    label: hook_supplied.label,
                    payload: hook_supplied.payload,
                    reason: synth.reason,
                }),
                // Neither classifier nor hook required a prompt → no gate.
                (None, None) => None,
            };
        // Prompt path: ask the embedder, map decision to allow/block. Fail-closed when no
        // prompt channel is configured.
        if let Some(prompt_req) = effective_prompt {
            let decision = match inner.options.on_control_plane_prompt.clone() {
                Some(prompt_hook) => prompt_hook(prompt_req.clone(), cancel.clone()).await,
                None => ControlPlanePromptDecision::Deny {
                    reason: Some(
                        "control-plane prompt required but no on_control_plane_prompt hook \
                         configured (fail-closed deny — see issue #110 design v0.2)"
                            .to_string(),
                    ),
                },
            };
            emit(
                inner,
                AgentEvent::ControlPlanePromptResolved {
                    tool_call_id: prompt_req.tool_call_id.clone(),
                    tool_name: prompt_req.tool_name.clone(),
                    args_hash: prompt_req.args_hash.clone(),
                    label: prompt_req.label.clone(),
                    decision: decision.as_audit_str().to_string(),
                    reason: match &decision {
                        ControlPlanePromptDecision::Deny { reason } => reason.clone(),
                        _ => None,
                    },
                },
                cancel,
            )
            .await;
            match decision {
                ControlPlanePromptDecision::Allow => {
                    // fall through to dispatch
                }
                ControlPlanePromptDecision::Deny { reason } => {
                    let reason = reason.unwrap_or_else(|| {
                        "tool call denied by user via control-plane prompt".to_string()
                    });
                    let result = AgentToolResult {
                        content: vec![UserContentBlock::text(reason)],
                        details: serde_json::Value::Null,
                        terminate: None,
                    };
                    prepared.push(PreparedCall::Blocked {
                        id: tool_id,
                        name: tool_name,
                        args,
                        result,
                    });
                    continue;
                }
                ControlPlanePromptDecision::Timeout => {
                    let result = AgentToolResult {
                        content: vec![UserContentBlock::text(
                            "control-plane prompt timed out — tool call denied".to_string(),
                        )],
                        details: serde_json::Value::Null,
                        terminate: None,
                    };
                    prepared.push(PreparedCall::Blocked {
                        id: tool_id,
                        name: tool_name,
                        args,
                        result,
                    });
                    continue;
                }
            }
        }

        prepared.push(PreparedCall::Run {
            id: tool_id,
            name: tool_name,
            args,
            tool,
        });
    }

    // Execute. For sequential we await one at a time; for parallel we spawn and join.
    let outcomes = match mode {
        ToolExecutionMode::Sequential => {
            let mut out = Vec::with_capacity(prepared.len());
            for call in prepared {
                out.push(run_one(inner.clone(), call, cancel.clone()).await);
            }
            out
        }
        ToolExecutionMode::Parallel => {
            let handles: Vec<_> = prepared
                .into_iter()
                .map(|call| {
                    let cancel = cancel.clone();
                    let inner = inner.clone();
                    tokio::spawn(async move { run_one(inner, call, cancel).await })
                })
                .collect();
            let mut out = Vec::with_capacity(handles.len());
            for h in handles {
                out.push(h.await.unwrap_or_else(|e| ToolOutcome {
                    id: String::new(),
                    name: String::new(),
                    args: serde_json::Value::Null,
                    result: AgentToolResult {
                        content: vec![UserContentBlock::text(format!("tool task join: {e}"))],
                        details: serde_json::Value::Null,
                        terminate: None,
                    },
                    is_error: true,
                }));
            }
            out
        }
    };

    // Post-process: run after_tool_call hooks (which may override), emit tool_execution_end,
    // build tool-result messages.
    let mut results = Vec::with_capacity(outcomes.len());
    let mut all_terminate = !outcomes.is_empty();
    let agent_context = snapshot_context(inner);
    for outcome in outcomes {
        let ToolOutcome {
            id,
            name,
            args,
            mut result,
            mut is_error,
        } = outcome;

        if let Some(hook) = inner.options.after_tool_call.clone() {
            let ctx = AfterToolCallContext {
                assistant_message: assistant.clone(),
                tool_call: theway_llm_provider::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: args.as_object().cloned().unwrap_or_default(),
                    thought_signature: None,
                },
                args: args.clone(),
                result: result.clone(),
                is_error,
                context: agent_context.clone(),
            };
            let patch = hook(ctx, cancel.clone()).await;
            if let Some(content) = patch.content {
                result.content = content;
            }
            if let Some(details) = patch.details {
                result.details = details;
            }
            if let Some(err) = patch.is_error {
                is_error = err;
            }
            if let Some(t) = patch.terminate {
                result.terminate = Some(t);
            }
        }

        if !result.terminate.unwrap_or(false) {
            all_terminate = false;
        }

        emit(
            inner,
            AgentEvent::ToolExecutionEnd {
                tool_call_id: id.clone(),
                tool_name: name.clone(),
                result: result.clone(),
                is_error,
            },
            cancel,
        )
        .await;

        results.push(ToolResultMessage {
            role: theway_llm_provider::ToolResultRole::ToolResult,
            tool_call_id: id,
            tool_name: name,
            content: result.content,
            details: Some(result.details),
            is_error,
            timestamp: chrono::Utc::now().timestamp_millis(),
        });
    }
    (results, all_terminate)
}

pub(super) enum PreparedCall {
    Run {
        id: String,
        name: String,
        args: serde_json::Value,
        tool: Option<Arc<dyn AgentTool>>,
    },
    Blocked {
        id: String,
        name: String,
        args: serde_json::Value,
        result: AgentToolResult,
    },
}

pub(super) struct ToolOutcome {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) args: serde_json::Value,
    pub(super) result: AgentToolResult,
    pub(super) is_error: bool,
}
