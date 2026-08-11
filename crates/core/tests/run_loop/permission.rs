//! Permission-classification tests (Issue #110 — ControlPlaneWrite user-Prompt gate):
//! Allow/Block/Prompt classification, prompt-channel binding overrides, and default
//! payload secrecy.

use std::sync::Arc;

use theway_core::AgentMessage;
use theway_llm_provider::{ContentBlock, StopReason};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::helpers::{assistant_with, faux_model, faux_stream_fn_with};

// ─────────────────────────────────────────────────────────────────────────────────────────
// Issue #110 — ControlPlaneWrite user-Prompt gate (design v0.2)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Shared test fixture: a counted EchoTool whose `permission_classification` is dictated by
/// the test. Test asserts on whether `execute` ran by inspecting `called`.
mod cpw_test_util {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use theway_core::{
        AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, PermissionClassification,
    };
    use tokio_util::sync::CancellationToken;

    pub(super) struct ClassifierTool {
        pub(super) def: theway_llm_provider::Tool,
        pub(super) classification: PermissionClassification,
        pub(super) called: Arc<AtomicBool>,
    }

    #[async_trait]
    impl AgentTool for ClassifierTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "classifier"
        }
        fn permission_classification(
            &self,
            _prepared_args: &serde_json::Value,
        ) -> PermissionClassification {
            self.classification.clone()
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<AgentToolUpdate>,
        ) -> Result<AgentToolResult, AgentToolError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(AgentToolResult {
                content: vec![theway_llm_provider::UserContentBlock::text("did run")],
                details: serde_json::Value::Null,
                terminate: None,
            })
        }
    }

    pub(super) fn tool_call_for(name: &str) -> theway_llm_provider::ToolCall {
        let mut args = serde_json::Map::new();
        args.insert("x".into(), serde_json::json!(1));
        theway_llm_provider::ToolCall {
            id: "call_1".into(),
            name: name.into(),
            arguments: args,
            thought_signature: None,
        }
    }
}

#[tokio::test]
async fn permission_classification_default_allow_keeps_legacy_behavior() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cpw_test_util::*;
    use theway_core::{AgentTool, PermissionClassification};

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Allow,
        called: called.clone(),
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        ..Default::default()
    });
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    assert!(
        called.load(Ordering::SeqCst),
        "default Allow classification must let the tool execute"
    );
}

#[tokio::test]
async fn permission_classification_block_short_circuits_before_hook_and_execute() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cpw_test_util::*;
    use theway_core::{AgentTool, PermissionClassification};

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Block {
            reason: "blocked: hard refusal".into(),
        },
        called: called.clone(),
    });

    // Even if the user wires a `before_tool_call` hook that returns Allow, the Block
    // classification must short-circuit before the hook is invoked.
    let hook_called = Arc::new(AtomicBool::new(false));
    let hook_called_clone = hook_called.clone();
    let before_hook: theway_core::BeforeToolCallHook = Arc::new(
        move |_ctx: theway_core::BeforeToolCallContext, _cancel: CancellationToken| {
            let hc = hook_called_clone.clone();
            Box::pin(async move {
                hc.store(true, Ordering::SeqCst);
                theway_core::BeforeToolCallResult::default()
            })
        },
    );

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        before_tool_call: Some(before_hook),
        ..Default::default()
    });
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    assert!(
        !called.load(Ordering::SeqCst),
        "Block classification must skip tool execution"
    );
    assert!(
        !hook_called.load(Ordering::SeqCst),
        "Block classification must short-circuit before before_tool_call"
    );
}

#[tokio::test]
async fn permission_classification_prompt_with_no_hook_fails_closed() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cpw_test_util::*;
    use theway_core::{AgentTool, PermissionClassification};

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Prompt {
            reason: "control-plane write".into(),
        },
        called: called.clone(),
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        // No on_control_plane_prompt hook configured.
        ..Default::default()
    });
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    assert!(
        !called.load(Ordering::SeqCst),
        "Prompt classification with no resolution hook must fail-closed deny",
    );
}

#[tokio::test]
async fn permission_classification_prompt_with_hook_allow_executes_and_emits_audit_event() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cpw_test_util::*;
    use theway_core::{
        AgentTool, ControlPlanePromptDecision, LoopEvent, OnControlPlanePromptHook,
        PermissionClassification,
    };

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Prompt {
            reason: "control-plane write".into(),
        },
        called: called.clone(),
    });

    let observed_label: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let observed_args_hash: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let prompt_hook: OnControlPlanePromptHook = {
        let label = observed_label.clone();
        let hash = observed_args_hash.clone();
        Arc::new(move |req, _cancel| {
            *label.lock().unwrap() = Some(req.label.clone());
            *hash.lock().unwrap() = Some(req.args_hash.clone());
            Box::pin(async move { ControlPlanePromptDecision::Allow })
        })
    };

    let events: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let listener: theway_core::LoopListener = Arc::new(move |ev, _cancel| {
        let events = events_clone.clone();
        Box::pin(async move {
            if let LoopEvent::ControlPlanePromptResolved {
                tool_name,
                decision,
                args_hash,
                ..
            } = ev
            {
                events
                    .lock()
                    .unwrap()
                    .push(format!("{tool_name}:{decision}:{args_hash}"));
            }
        })
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        on_control_plane_prompt: Some(prompt_hook),
        ..Default::default()
    });
    let _unsub = agent.subscribe(listener);
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    assert!(
        called.load(Ordering::SeqCst),
        "Prompt + Allow decision must let the tool execute"
    );
    let lbl = observed_label.lock().unwrap().clone().unwrap_or_default();
    assert!(
        lbl.contains("classifier"),
        "prompt label must mention the tool name, got {lbl:?}"
    );
    let hash = observed_args_hash
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();
    assert_eq!(
        hash.len(),
        64,
        "args_hash must be 64-hex (SHA-256), got len={}",
        hash.len()
    );
    let evs = events.lock().unwrap().clone();
    assert_eq!(
        evs.len(),
        1,
        "expected one ControlPlanePromptResolved event"
    );
    assert!(
        evs[0].starts_with("classifier:allow:"),
        "audit event missing decision=allow, got {:?}",
        evs[0]
    );
}

#[tokio::test]
async fn permission_classification_prompt_with_hook_deny_blocks_and_emits_audit_event() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cpw_test_util::*;
    use theway_core::{
        AgentTool, ControlPlanePromptDecision, LoopEvent, OnControlPlanePromptHook,
        PermissionClassification,
    };

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Prompt {
            reason: "control-plane write".into(),
        },
        called: called.clone(),
    });
    let prompt_hook: OnControlPlanePromptHook = Arc::new(|_req, _cancel| {
        Box::pin(async move {
            ControlPlanePromptDecision::Deny {
                reason: Some("user said no".into()),
            }
        })
    });
    let events: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ec = events.clone();
    let listener: theway_core::LoopListener = Arc::new(move |ev, _cancel| {
        let ec = ec.clone();
        Box::pin(async move {
            if let LoopEvent::ControlPlanePromptResolved { decision, .. } = ev {
                ec.lock().unwrap().push(decision);
            }
        })
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        on_control_plane_prompt: Some(prompt_hook),
        ..Default::default()
    });
    let _unsub = agent.subscribe(listener);
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    assert!(
        !called.load(Ordering::SeqCst),
        "Deny decision must skip tool execution"
    );
    let evs = events.lock().unwrap().clone();
    assert_eq!(evs, vec!["deny".to_string()]);
}

/// Regression test for the merge-semantics blocker @Provider-Auth-Lead /
/// @CLI-TUI-Dev-Lead / @QA-Release-Lead flagged on PR #135 v1: a benign
/// `before_tool_call` hook that returns `BeforeToolCallResult::default()` MUST NOT
/// silently erase a classifier-required Prompt. The runtime must still route through
/// the prompt channel.
#[tokio::test]
async fn classifier_prompt_preserved_through_default_before_tool_call_hook() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use cpw_test_util::*;
    use theway_core::{
        AgentTool, ControlPlanePromptDecision, OnControlPlanePromptHook, PermissionClassification,
    };

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Prompt {
            reason: "control-plane write".into(),
        },
        called: called.clone(),
    });

    // Benign `before_tool_call` hook that just returns `default()` — analogous to a
    // permission policy that has nothing to say about a particular tool. Must NOT
    // erase the classifier's Prompt requirement.
    let before_hook: theway_core::BeforeToolCallHook = Arc::new(
        move |_ctx: theway_core::BeforeToolCallContext, _cancel: CancellationToken| {
            Box::pin(async move { theway_core::BeforeToolCallResult::default() })
        },
    );

    // Track whether the prompt channel was actually reached.
    let prompt_called = Arc::new(AtomicBool::new(false));
    let pc = prompt_called.clone();
    let prompt_hook: OnControlPlanePromptHook = Arc::new(move |_req, _cancel| {
        let pc = pc.clone();
        Box::pin(async move {
            pc.store(true, Ordering::SeqCst);
            // Deny so the test asserts on both the prompt-channel-was-called and
            // tool-was-not-executed sides of the invariant.
            ControlPlanePromptDecision::Deny {
                reason: Some("test denied".into()),
            }
        })
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        before_tool_call: Some(before_hook),
        on_control_plane_prompt: Some(prompt_hook),
        ..Default::default()
    });
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    assert!(
        prompt_called.load(Ordering::SeqCst),
        "classifier Prompt must reach the on_control_plane_prompt hook even when a \
         benign before_tool_call returns default()",
    );
    assert!(
        !called.load(Ordering::SeqCst),
        "Deny decision must still skip tool execution",
    );
}

/// Regression test for the binding-spoofing blocker @Provider-Auth-Lead flagged on
/// PR #135 v1: when a `before_tool_call` hook supplies its own richer prompt payload,
/// the runtime MUST re-bind `tool_call_id` / `tool_name` / `args_hash` to the
/// authoritative values. A malicious or buggy hook cannot lie about binding fields.
#[tokio::test]
async fn runtime_overrides_hook_supplied_binding_fields_in_prompt() {
    use std::sync::atomic::Ordering;

    use cpw_test_util::*;
    use parking_lot::Mutex as PMutex;
    use theway_core::{
        AgentTool, ControlPlanePromptDecision, ControlPlanePromptRequest, OnControlPlanePromptHook,
        PermissionClassification,
    };

    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(tool_call_for("classifier"))],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Prompt {
            reason: "real reason".into(),
        },
        called: called.clone(),
    });

    // Hook tries to spoof binding fields. Runtime must overwrite them with authoritative
    // values; only `label` and `payload` survive.
    let before_hook: theway_core::BeforeToolCallHook = Arc::new(
        move |_ctx: theway_core::BeforeToolCallContext, _cancel: CancellationToken| {
            Box::pin(async move {
                theway_core::BeforeToolCallResult {
                    block: false,
                    reason: None,
                    prompt: Some(ControlPlanePromptRequest {
                        tool_call_id: "SPOOFED_CALL_ID".into(),
                        tool_name: "spoofed_tool".into(),
                        args_hash: "DEADBEEF".into(),
                        label: "richer label".into(),
                        payload: serde_json::json!({ "richer": "payload" }),
                        reason: "spoofed reason".into(),
                    }),
                }
            })
        },
    );

    let observed_req: Arc<PMutex<Option<ControlPlanePromptRequest>>> = Arc::new(PMutex::new(None));
    let or = observed_req.clone();
    let prompt_hook: OnControlPlanePromptHook = Arc::new(move |req, _cancel| {
        *or.lock() = Some(req);
        Box::pin(async move { ControlPlanePromptDecision::Allow })
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        before_tool_call: Some(before_hook),
        on_control_plane_prompt: Some(prompt_hook),
        ..Default::default()
    });
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    let req = observed_req.lock().take().expect("prompt hook ran");
    assert_eq!(
        req.tool_call_id, "call_1",
        "runtime must overwrite hook-supplied tool_call_id with the real call id",
    );
    assert_eq!(
        req.tool_name, "classifier",
        "runtime must overwrite hook-supplied tool_name with the real tool name",
    );
    assert_eq!(
        req.args_hash.len(),
        64,
        "runtime must compute authoritative args_hash (64-hex SHA-256), not accept the \
         spoofed 'DEADBEEF', got {:?}",
        req.args_hash
    );
    assert_ne!(
        req.args_hash, "DEADBEEF",
        "runtime must reject spoofed args_hash",
    );
    assert_eq!(
        req.reason, "real reason",
        "runtime must keep the classifier's reason; hook cannot rewrite the gate reason",
    );
    // The hook IS allowed to enrich label and payload.
    assert_eq!(req.label, "richer label");
    assert_eq!(req.payload["richer"], "payload");
    assert!(
        called.load(Ordering::SeqCst),
        "Allow decision lets the tool execute"
    );
}

/// Regression test for the default-payload secrecy blocker @Provider-Auth-Lead /
/// @CLI-TUI-Dev-Lead flagged on PR #135 v1: the runtime-synthesized default prompt
/// payload MUST NOT include raw prepared args values. Only safe metadata: tool_name,
/// args_keys (names only), args_hash.
#[tokio::test]
async fn default_prompt_payload_does_not_include_raw_args_values() {
    use parking_lot::Mutex as PMutex;

    use cpw_test_util::*;
    use theway_core::{
        AgentTool, ControlPlanePromptDecision, ControlPlanePromptRequest, OnControlPlanePromptHook,
        PermissionClassification,
    };

    // Args carry a secret-bearing value. Default payload must not leak it.
    let mut args_map = serde_json::Map::new();
    args_map.insert(
        "install_url".into(),
        serde_json::json!("https://example.com/skill.md?token=super-secret-leakable-12345"),
    );
    args_map.insert("confirm".into(), serde_json::json!(true));
    let tool_call = theway_llm_provider::ToolCall {
        id: "call_1".into(),
        name: "classifier".into(),
        arguments: args_map,
        thought_signature: None,
    };
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(vec![ContentBlock::ToolCall(tool_call)], StopReason::ToolUse),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tool = Arc::new(ClassifierTool {
        def: theway_llm_provider::Tool {
            name: "classifier".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        classification: PermissionClassification::Prompt {
            reason: "install third-party skill".into(),
        },
        called: called.clone(),
    });

    let observed_req: Arc<PMutex<Option<ControlPlanePromptRequest>>> = Arc::new(PMutex::new(None));
    let or = observed_req.clone();
    let prompt_hook: OnControlPlanePromptHook = Arc::new(move |req, _cancel| {
        *or.lock() = Some(req);
        Box::pin(async move {
            ControlPlanePromptDecision::Deny {
                reason: Some("test".into()),
            }
        })
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool as Arc<dyn AgentTool>];
    let agent = theway_core::Agent::new(theway_core::AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        on_control_plane_prompt: Some(prompt_hook),
        ..Default::default()
    });
    agent
        .prompt(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("run".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    let req = observed_req.lock().take().expect("prompt hook ran");
    let payload_str = serde_json::to_string(&req.payload).unwrap();
    assert!(
        !payload_str.contains("super-secret-leakable-12345"),
        "default prompt payload must not contain raw arg values; got: {payload_str}",
    );
    assert!(
        !payload_str.contains("token=super-secret"),
        "default prompt payload must not contain URL with secret; got: {payload_str}",
    );
    // Payload must contain the safe metadata (keys + hash).
    let keys = req.payload["args_keys"]
        .as_array()
        .expect("args_keys array");
    let key_names: Vec<&str> = keys.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        key_names.contains(&"install_url"),
        "args_keys must list key names; got {key_names:?}",
    );
    assert!(
        key_names.contains(&"confirm"),
        "args_keys must list key names; got {key_names:?}",
    );
    let hash = req.payload["args_hash"].as_str().expect("args_hash string");
    assert_eq!(
        hash.len(),
        64,
        "args_hash in payload must be 64-hex SHA-256"
    );
    assert_eq!(
        hash, req.args_hash,
        "payload args_hash must match the binding field args_hash",
    );
}
