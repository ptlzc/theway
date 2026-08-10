//! Control-plane prompt audit tests (split out of control_plane.rs).

use super::helpers::faux_model;
use super::turn_end::{PromptingTool, read_custom_entries};
use super::*;

fn faux_stream_fn_classifier_then_stop() -> theway_core::StreamFn {
    use std::sync::Mutex as SMutex;
    let counter = Arc::new(SMutex::new(0u32));
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        let counter = counter.clone();
        tokio::spawn(async move {
            let n = {
                let mut c = counter.lock().unwrap();
                let v = *c;
                *c = c.saturating_add(1);
                v
            };
            let msg = if n == 0 {
                let mut args = serde_json::Map::new();
                args.insert("k".into(), serde_json::json!("v"));
                theway_llm_provider::AssistantMessage {
                    role: theway_llm_provider::AssistantRole::Assistant,
                    content: vec![theway_llm_provider::ContentBlock::ToolCall(
                        theway_llm_provider::ToolCall {
                            id: "call_x".into(),
                            name: "prompter".into(),
                            arguments: args,
                            thought_signature: None,
                        },
                    )],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: theway_llm_provider::Usage::default(),
                    stop_reason: theway_llm_provider::StopReason::ToolUse,
                    error_message: None,
                    timestamp: 0,
                }
            } else {
                theway_llm_provider::AssistantMessage {
                    role: theway_llm_provider::AssistantRole::Assistant,
                    content: vec![theway_llm_provider::ContentBlock::text("ok done")],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: theway_llm_provider::Usage::default(),
                    stop_reason: theway_llm_provider::StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                }
            };
            let reason = match msg.stop_reason {
                theway_llm_provider::StopReason::ToolUse => {
                    theway_llm_provider::DoneReason::ToolUse
                }
                _ => theway_llm_provider::DoneReason::Stop,
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason,
                message: msg,
            });
        });
        stream
    })
}

#[tokio::test]
async fn control_plane_prompt_allow_writes_audit_entry() {
    use theway_core::{AgentTool, ControlPlanePromptDecision, OnControlPlanePromptHook};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];
    let prompt_hook: OnControlPlanePromptHook =
        Arc::new(|_req, _cancel| Box::pin(async move { ControlPlanePromptDecision::Allow }));
    opts.on_control_plane_prompt = Some(prompt_hook);

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(
        audits.len(),
        1,
        "expected exactly one control_plane_prompt audit, got {}",
        audits.len()
    );
    let audit = &audits[0];
    assert_eq!(audit["schema_version"], 1);
    assert_eq!(audit["tool_call_id"], "call_x");
    assert_eq!(audit["tool_name"], "prompter");
    assert_eq!(audit["decision"], "allow");
    let hash = audit["args_hash"].as_str().expect("args_hash string");
    assert_eq!(hash.len(), 64, "args_hash must be 64-hex SHA-256");
    assert!(
        audit["label"]
            .as_str()
            .map(|s| s.contains("prompter"))
            .unwrap_or(false),
        "label must mention the tool name, got {:?}",
        audit["label"]
    );
    assert!(audit["at"].is_string(), "at must be a string timestamp");
}

#[tokio::test]
async fn control_plane_prompt_deny_writes_audit_with_reason() {
    use theway_core::{AgentTool, ControlPlanePromptDecision, OnControlPlanePromptHook};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];
    let prompt_hook: OnControlPlanePromptHook = Arc::new(|_req, _cancel| {
        Box::pin(async move {
            ControlPlanePromptDecision::Deny {
                reason: Some("user pressed N".into()),
            }
        })
    });
    opts.on_control_plane_prompt = Some(prompt_hook);

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["decision"], "deny");
    assert_eq!(audits[0]["reason"], "user pressed N");
}

#[tokio::test]
async fn control_plane_prompt_no_hook_writes_audit_with_failclosed_deny() {
    use theway_core::AgentTool;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];
    // No on_control_plane_prompt hook — runtime fails closed AND still writes the
    // audit recording the rejection.

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["decision"], "deny");
    let reason = audits[0]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("no on_control_plane_prompt hook"),
        "no-hook deny reason should mention missing hook, got {reason:?}"
    );
}

#[tokio::test]
async fn control_plane_prompt_audit_caps_oversized_label() {
    use theway_core::{
        AgentTool, BeforeToolCallContext, BeforeToolCallHook, BeforeToolCallResult,
        ControlPlanePromptDecision, ControlPlanePromptRequest, OnControlPlanePromptHook,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];

    // Hook supplies an oversized label. Runtime keeps authoritative binding fields
    // (covered by sub-PR 1 regression test) AND the harness audit caps the label
    // to ≤ 200 chars on a char boundary, ending with the truncation marker.
    let oversized = "x".repeat(1000);
    let oversized_clone = oversized.clone();
    let before_hook: BeforeToolCallHook = Arc::new(
        move |_ctx: BeforeToolCallContext, _cancel: tokio_util::sync::CancellationToken| {
            let label = oversized_clone.clone();
            Box::pin(async move {
                BeforeToolCallResult {
                    block: false,
                    reason: None,
                    prompt: Some(ControlPlanePromptRequest {
                        tool_call_id: "spoofed".into(),
                        tool_name: "spoofed".into(),
                        args_hash: "deadbeef".into(),
                        label,
                        payload: serde_json::json!({}),
                        reason: "spoofed reason".into(),
                    }),
                }
            })
        },
    );
    opts.before_tool_call = Some(before_hook);
    let prompt_hook: OnControlPlanePromptHook =
        Arc::new(|_req, _cancel| Box::pin(async move { ControlPlanePromptDecision::Allow }));
    opts.on_control_plane_prompt = Some(prompt_hook);

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(audits.len(), 1);
    let label = audits[0]["label"].as_str().expect("label string");
    let label_chars = label.chars().count();
    assert!(
        label_chars <= 200,
        "audit label must be capped at 200 chars, got {label_chars}",
    );
    assert!(
        label.ends_with('…'),
        "capped audit label should end with truncation marker, got tail of {label:?}",
    );
}
