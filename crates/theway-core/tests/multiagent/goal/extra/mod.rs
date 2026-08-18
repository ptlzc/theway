//! Extra tests for `multiagent::goal` — bridged through `goal_extra_tests`
//! because the existing `multiagent/goal` test module was already occupied.

use std::sync::Arc;

use super::super::*;
use crate::agent::assembly::{AgentHarness, AgentHarnessOptions};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::Session;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, Message as PiMessage, ToolResultMessage, ToolResultRole,
    UserContent, UserContentBlock, UserMessage, UserRole,
};

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_msg(blocks: Vec<ContentBlock>) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: blocks,
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
    }))
}

fn tool_result_msg(tool_name: &str, blocks: Vec<UserContentBlock>) -> AgentMessage {
    AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: "t1".into(),
        tool_name: tool_name.into(),
        content: blocks,
        details: None,
        is_error: false,
        timestamp: 0,
    }))
}

#[test]
fn goal_status_as_str_is_snake_case() {
    assert_eq!(GoalStatus::Pursuing.as_str(), "pursuing");
    assert_eq!(GoalStatus::BudgetLimited.as_str(), "budget_limited");
}

#[test]
fn goal_state_active_is_true_for_pursuing_paused_budget_limited() {
    for status in [
        GoalStatus::Pursuing,
        GoalStatus::Paused,
        GoalStatus::BudgetLimited,
    ] {
        let state = GoalState {
            condition: "c".into(),
            status,
            iterations: 0,
            last_reason: None,
            updated_at: "t".into(),
        };
        assert!(state.active());
    }

    let achieved = GoalState {
        condition: "c".into(),
        status: GoalStatus::Achieved,
        iterations: 0,
        last_reason: None,
        updated_at: "t".into(),
    };
    assert!(!achieved.active());
    let cleared = GoalState {
        condition: "c".into(),
        status: GoalStatus::Cleared,
        iterations: 0,
        last_reason: None,
        updated_at: "t".into(),
    };
    assert!(!cleared.active());
}

#[test]
fn latest_from_entries_finds_latest_goal_state_and_skips_other_entries() {
    let entries = vec![
        SessionTreeEntry::Custom {
            id: "1".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "goal_state".into(),
            data: Some(serde_json::json!({
                "condition": "first",
                "status": "pursuing",
                "iterations": 0,
                "updated_at": "t"
            })),
        },
        SessionTreeEntry::Custom {
            id: "2".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "something_else".into(),
            data: Some(serde_json::json!({"not": "goal"})),
        },
        SessionTreeEntry::Custom {
            id: "3".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "goal_state".into(),
            data: Some(serde_json::json!({
                "condition": "second",
                "status": "paused",
                "iterations": 2,
                "last_reason": "waiting",
                "updated_at": "t"
            })),
        },
        SessionTreeEntry::Custom {
            id: "4".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "goal_state".into(),
            data: None,
        },
    ];

    let state = latest_from_entries(&entries).expect("latest goal state");
    assert_eq!(state.condition, "second");
    assert_eq!(state.status, GoalStatus::Paused);
    assert_eq!(state.iterations, 2);
}

#[test]
fn transcript_from_messages_bounds_and_joins_text() {
    let messages = vec![
        user_msg("hello"),
        assistant_msg(vec![ContentBlock::text("assistant text")]),
        tool_result_msg("grep", vec![UserContentBlock::text("3 matches")]),
    ];

    let transcript = transcript_from_messages(&messages, 10_000);

    assert!(transcript.contains("User: hello"));
    assert!(transcript.contains("Assistant: assistant text"));
    assert!(transcript.contains("ToolResult(grep error=false): 3 matches"));
}

#[test]
fn transcript_from_messages_truncates_to_last_chars() {
    let messages = vec![user_msg(&"x".repeat(500))];

    let transcript = transcript_from_messages(&messages, 100);

    assert!(transcript.starts_with("[transcript truncated to last 100 chars]"));
    assert_eq!(transcript.chars().count(), "[transcript truncated to last 100 chars]\n".len() + 100);
}

#[test]
fn agent_message_text_handles_content_blocks_and_custom() {
    let user = user_msg("hi");
    assert_eq!(agent_message_text(&user).unwrap(), "User: hi");

    let assistant = assistant_msg(vec![
        ContentBlock::text("text part"),
        ContentBlock::Thinking(theway_llm_provider::ThinkingContent {
            thinking: "thinking part".into(),
            thinking_signature: None,
            redacted: false,
        }),
        ContentBlock::ToolCall(theway_llm_provider::ToolCall {
            id: "t".into(),
            name: "read".into(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        }),
    ]);
    let text = agent_message_text(&assistant).unwrap();
    assert!(text.starts_with("Assistant: "));
    assert!(text.contains("text part"));
    assert!(text.contains("thinking part"));
    assert!(text.contains("read"));

    let tool_result = tool_result_msg(
        "grep",
        vec![
            UserContentBlock::text("line one"),
            UserContentBlock::Image(theway_llm_provider::ImageContent {
                data: "base64".into(),
                mime_type: "image/png".into(),
            }),
        ],
    );
    let text = agent_message_text(&tool_result).unwrap();
    assert!(text.contains("line one"));
    assert!(!text.contains("base64"));

    let custom = AgentMessage::Custom(crate::types::CustomMessage {
        role: "custom".into(),
        timestamp: 0,
        payload: serde_json::json!({"a": 1}),
    });
    assert_eq!(agent_message_text(&custom), None);
}

#[test]
fn user_content_text_renders_blocks_and_image_placeholder() {
    assert_eq!(user_content_text(&UserContent::Text("plain".into())), "plain");
    let blocks = UserContent::Blocks(vec![
        UserContentBlock::text("a"),
        UserContentBlock::Image(theway_llm_provider::ImageContent {
            data: "base64".into(),
            mime_type: "image/png".into(),
        }),
    ]);
    assert_eq!(user_content_text(&blocks), "a\n[image]");
}

#[test]
fn evaluator_user_prompt_contains_condition_and_transcript() {
    let prompt = evaluator_user_prompt("tests pass", "TRANSCRIPT");
    assert!(prompt.contains("tests pass"));
    assert!(prompt.contains("TRANSCRIPT"));
}

#[test]
fn evaluator_system_prompt_documents_json_contract() {
    let prompt = evaluator_system_prompt();
    assert!(prompt.contains("{\"ok\": true"));
    assert!(prompt.contains("insufficient evidence in transcript"));
}

#[test]
fn parse_decision_accepts_valid_json_and_extracts_fenced_json() {
    let decision = parse_decision("{\"ok\":true,\"reason\":\"evidence\"}").unwrap();
    assert!(decision.ok);
    assert_eq!(decision.reason, "evidence");

    let fenced = parse_decision("```json\n{\"ok\":false,\"reason\":\"not yet\"}\n```").unwrap();
    assert!(!fenced.ok);
    assert_eq!(fenced.reason, "not yet");
}

#[test]
fn parse_decision_rejects_invalid_json_and_empty_reason() {
    let err = parse_decision("not json").unwrap_err();
    assert!(err.contains("invalid JSON"));
    assert!(err.contains("not json"));

    let err = parse_decision("{\"ok\":true,\"reason\":\"   \"}").unwrap_err();
    assert!(err.contains("empty reason"));
}

#[test]
fn continuation_prompt_contains_condition_and_missing_evidence() {
    let prompt = continuation_prompt("tests pass", "no tests yet");
    assert!(prompt.contains("tests pass"));
    assert!(prompt.contains("no tests yet"));
    assert!(prompt.contains("Do not claim completion"));
}

#[test]
fn tail_chars_returns_original_when_within_limit() {
    assert_eq!(tail_chars("abcdef", 10), "abcdef");
}

#[test]
fn goal_payload_includes_status_condition_reason_and_budget() {
    let state = GoalState {
        condition: "c".into(),
        status: GoalStatus::Pursuing,
        iterations: 3,
        last_reason: Some("missing".into()),
        updated_at: "t".into(),
    };
    let payload = goal_payload(&state, Some(false));
    assert_eq!(payload["goal_status"], serde_json::json!("pursuing"));
    assert_eq!(payload["condition"], serde_json::json!("c"));
    assert_eq!(payload["ok"], serde_json::json!(false));
    assert_eq!(payload["reason"], serde_json::json!("missing"));
    assert_eq!(payload["iterations"], serde_json::json!(3));
    assert_eq!(payload["max_continuations"], serde_json::json!(MAX_CONTINUATIONS));
}

#[test]
fn pause_decision_returns_pause_action_with_payload() {
    let state = GoalState {
        condition: "c".into(),
        status: GoalStatus::Pursuing,
        iterations: 0,
        last_reason: None,
        updated_at: "t".into(),
    };
    let decision = pause_decision("stop now".into(), &state);
    assert!(matches!(
        decision.action,
        TurnEndAction::Pause { ref reason } if reason == "stop now"
    ));
    assert!(decision.payload.is_some());
}

#[tokio::test]
async fn set_current_pause_resume_clear_roundtrip() {
    let harness = Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(MemorySessionStorage::new())),
    )));

    assert!(current(&harness).await.is_none());
    assert!(pause(&harness).await.is_err());

    let state = set(&harness, "tests pass".into()).await.unwrap();
    assert_eq!(state.status, GoalStatus::Pursuing);
    assert_eq!(state.condition, "tests pass");

    let current_state = current(&harness).await.unwrap();
    assert_eq!(current_state.condition, "tests pass");

    let paused = pause(&harness).await.unwrap();
    assert_eq!(paused.status, GoalStatus::Paused);

    let resumed = resume(&harness).await.unwrap();
    assert_eq!(resumed.status, GoalStatus::Pursuing);

    // Resume while already pursuing is an error.
    let err = resume(&harness).await.unwrap_err();
    assert!(err.contains("not paused"));

    let cleared = clear(&harness).await.unwrap();
    assert_eq!(cleared.status, GoalStatus::Cleared);
    assert!(current(&harness).await.is_none());
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// evaluate_stop_hook success / terminal-path coverage
// ──────────────────────────────────────────────────────────────────────────────────────────

fn harness() -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(MemorySessionStorage::new())),
    )))
}

fn decision_stream(text: &'static str) -> crate::types::StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) =
            theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
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
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

fn goal_resolver() -> crate::multiagent::types::AgentRunResolver {
    let launch = crate::multiagent::types::AgentRunParams {
        name: "goal-evaluator",
        description: "judge",
        system_prompt: evaluator_system_prompt(),
        max_iterations: 1,
    };
    Arc::new(move |name: &str| (name == "goal-evaluator").then_some(launch))
}

fn ctx_with(transcript: Vec<AgentMessage>) -> OnTurnEndContext {
    OnTurnEndContext {
        transcript,
        continuation_count: 0,
        last_user_prompt: Some("finish".into()),
    }
}

#[tokio::test]
async fn evaluate_stop_hook_returns_stop_when_goal_achieved() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    let engine = Arc::new(DagEngine::new());

    let decision = evaluate_stop_hook(
        h.clone(),
        engine.clone(),
        goal_resolver(),
        AgentJobRegistry::new(),
        Some(decision_stream(r#"{"ok":true,"reason":"all tests pass"}"#)),
        ctx_with(vec![user_msg("hi")]),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(matches!(decision.action, TurnEndAction::Stop));
    let state = current(&h).await.unwrap();
    assert_eq!(state.status, GoalStatus::Achieved);
    assert_eq!(state.iterations, 1);
    assert_eq!(state.last_reason.as_deref(), Some("all tests pass"));
    assert!(decision.payload.is_some());
}

#[tokio::test]
async fn evaluate_stop_hook_returns_continue_when_not_achieved() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    let engine = Arc::new(DagEngine::new());

    let decision = evaluate_stop_hook(
        h.clone(),
        engine.clone(),
        goal_resolver(),
        AgentJobRegistry::new(),
        Some(decision_stream(r#"{"ok":false,"reason":"missing evidence"}"#)),
        ctx_with(vec![user_msg("hi")]),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    match decision.action {
        TurnEndAction::Continue { ref prompt } => {
            assert!(prompt.contains("finish"));
            assert!(prompt.contains("missing evidence"));
        }
        other => panic!("expected Continue, got {other:?}"),
    }
    let state = current(&h).await.unwrap();
    assert_eq!(state.status, GoalStatus::Pursuing);
    assert_eq!(state.iterations, 1);
}

#[tokio::test]
async fn evaluate_stop_hook_budget_limits_after_max_continuations() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    // Pre-set the persisted state one iteration below the cap.
    let mut state = current(&h).await.unwrap();
    state.iterations = MAX_CONTINUATIONS - 1;
    append_state(&h, &state).await.unwrap();
    let engine = Arc::new(DagEngine::new());

    let decision = evaluate_stop_hook(
        h.clone(),
        engine.clone(),
        goal_resolver(),
        AgentJobRegistry::new(),
        Some(decision_stream(r#"{"ok":false,"reason":"still missing"}"#)),
        ctx_with(vec![user_msg("hi")]),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    match decision.action {
        TurnEndAction::Pause { ref reason } => {
            assert!(reason.contains("continuation limit reached"));
        }
        other => panic!("expected Pause, got {other:?}"),
    }
    let state = current(&h).await.unwrap();
    assert_eq!(state.status, GoalStatus::BudgetLimited);
    assert_eq!(state.iterations, MAX_CONTINUATIONS);
}

#[tokio::test]
async fn evaluate_stop_hook_pauses_on_invalid_evaluator_json() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    let engine = Arc::new(DagEngine::new());

    let decision = evaluate_stop_hook(
        h.clone(),
        engine.clone(),
        goal_resolver(),
        AgentJobRegistry::new(),
        Some(decision_stream("not-json")),
        ctx_with(vec![user_msg("hi")]),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    match decision.action {
        TurnEndAction::Pause { ref reason } => {
            assert!(reason.contains("goal evaluator failed"), "{reason}");
        }
        other => panic!("expected Pause, got {other:?}"),
    }
    let state = current(&h).await.unwrap();
    assert_eq!(state.status, GoalStatus::Paused);
}
