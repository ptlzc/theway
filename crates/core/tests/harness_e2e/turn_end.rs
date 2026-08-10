//! OnTurnEnd hook tests (split out of control_plane.rs).

use super::helpers::{faux_model, faux_stream_fn};
use super::*;

fn faux_stream_fn_sequence(texts: Vec<&'static str>) -> StreamFn {
    let cursor = Arc::new(std::sync::Mutex::new(0usize));
    let texts = Arc::new(texts);
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let texts = texts.clone();
        let cursor = cursor.clone();
        tokio::spawn(async move {
            let idx = {
                let mut c = cursor.lock().unwrap();
                let i = *c;
                *c = c.saturating_add(1);
                i
            };
            let text = *texts.get(idx).unwrap_or(&"<exhausted>");
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

fn collect_turn_end_decisions(
    received: &Arc<parking_lot::Mutex<Vec<HarnessEvent>>>,
) -> Vec<(&'static str, u32, Option<String>)> {
    received
        .lock()
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::TurnEnded {
                decision,
                continuation_count,
                reason,
                ..
            } => Some((*decision, *continuation_count, reason.clone())),
            _ => None,
        })
        .collect()
}

pub(crate) async fn read_custom_entries(
    storage: Arc<MemorySessionStorage>,
    kind: &str,
) -> Vec<serde_json::Value> {
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let entries = session.branch(None).await.unwrap();
    entries
        .into_iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == kind => data,
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn on_turn_end_hook_unset_keeps_legacy_single_cycle_behavior() {
    use parking_lot::Mutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("only-turn"));
    // No on_turn_end set — legacy path.

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("hi").await.unwrap();

    let turn_end_count = received
        .lock()
        .iter()
        .filter(|e| matches!(e, HarnessEvent::TurnEnded { .. }))
        .count();
    assert_eq!(turn_end_count, 0, "no TurnEnded event when hook is unset",);

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert!(
        audits.is_empty(),
        "no turn_end_decision audit when hook is unset"
    );
}

#[tokio::test]
async fn on_turn_end_hook_noop_writes_no_audit_no_event() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("just answering"));
    // Hook is permanently registered (e.g. `/goal` always-on hook), but
    // returns Noop when there's no active goal — should look identical to
    // "no hook configured" from the session's point of view.
    let invocation_count = Arc::new(Mutex::new(0u32));
    let ic = invocation_count.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let ic = ic.clone();
        Box::pin(async move {
            *ic.lock() += 1;
            TurnEndDecision {
                action: TurnEndAction::Noop,
                payload: Some(serde_json::json!({ "ignored": true })),
            }
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("hi").await.unwrap();

    assert_eq!(
        *invocation_count.lock(),
        1,
        "hook fires exactly once per turn even when it returns Noop",
    );
    let turn_end_count = received
        .lock()
        .iter()
        .filter(|e| matches!(e, HarnessEvent::TurnEnded { .. }))
        .count();
    assert_eq!(turn_end_count, 0, "Noop emits no TurnEnded event");

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert!(audits.is_empty(), "Noop writes no turn_end_decision audit");
}

#[tokio::test]
async fn on_turn_end_hook_stop_emits_event_and_audits_payload() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndContext, OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let observed_ctx: Arc<Mutex<Option<OnTurnEndContext>>> = Arc::new(Mutex::new(None));
    let observed_ctx_for_hook = observed_ctx.clone();

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("first-and-only"));
    let hook: OnTurnEndHook = Arc::new(move |ctx, _cancel| {
        let observed = observed_ctx_for_hook.clone();
        Box::pin(async move {
            *observed.lock() = Some(ctx);
            TurnEndDecision {
                action: TurnEndAction::Stop,
                payload: Some(serde_json::json!({ "kind": "goal_achieved" })),
            }
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("what is 2+2?").await.unwrap();

    let decisions = collect_turn_end_decisions(&received);
    assert_eq!(
        decisions,
        vec![("stop", 0, None)],
        "exactly one stop event with continuation_count=0"
    );

    let ctx = observed_ctx.lock().take().unwrap();
    assert_eq!(ctx.continuation_count, 0);
    assert_eq!(
        ctx.last_user_prompt.as_deref(),
        Some("what is 2+2?"),
        "hook sees the originating user prompt"
    );
    assert!(
        !ctx.transcript.is_empty(),
        "hook sees a non-empty transcript snapshot"
    );

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert_eq!(audits.len(), 1, "exactly one audit entry");
    assert_eq!(audits[0]["decision"], "stop");
    assert_eq!(audits[0]["continuation_count"], 0);
    assert_eq!(audits[0]["payload"]["kind"], "goal_achieved");
}

#[tokio::test]
async fn on_turn_end_continue_runs_second_turn_then_stops() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_sequence(vec!["first-turn", "second-turn"]));
    let call_count = Arc::new(Mutex::new(0u32));
    let cc = call_count.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let cc = cc.clone();
        Box::pin(async move {
            let n = {
                let mut g = cc.lock();
                *g = g.saturating_add(1);
                *g
            };
            if n == 1 {
                TurnEndDecision {
                    action: TurnEndAction::Continue {
                        prompt: "now do step 2".into(),
                    },
                    payload: Some(serde_json::json!({ "iter": 1 })),
                }
            } else {
                TurnEndDecision {
                    action: TurnEndAction::Stop,
                    payload: None,
                }
            }
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("start").await.unwrap();

    let decisions = collect_turn_end_decisions(&received);
    assert_eq!(
        decisions,
        vec![("continue", 1, None), ("stop", 1, None),],
        "continue then stop, post-decision counts: 1 then 1",
    );

    // Two audit entries with matching payloads.
    let audits = read_custom_entries(storage.clone(), "turn_end_decision").await;
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0]["decision"], "continue");
    assert_eq!(audits[0]["next_prompt_preview"], "now do step 2");
    assert_eq!(audits[0]["payload"]["iter"], 1);
    assert_eq!(audits[1]["decision"], "stop");
    assert!(audits[1]["payload"].is_null());

    // The transcript should have the original user msg + assistant + continuation user msg + assistant.
    let agent_state_messages = {
        let s = harness.agent().state();
        s.messages
            .iter()
            .map(|m| match m {
                AgentMessage::Llm(theway_llm_provider::Message::User(u)) => match &u.content {
                    theway_llm_provider::UserContent::Text(t) => format!("user:{t}"),
                    theway_llm_provider::UserContent::Blocks(_) => "user:<blocks>".into(),
                },
                AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => {
                    let text: String = a
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    format!("assistant:{text}")
                }
                AgentMessage::Llm(_) => "other-llm".into(),
                AgentMessage::Custom(_) => "custom".into(),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        agent_state_messages,
        vec![
            "user:start",
            "assistant:first-turn",
            "user:now do step 2",
            "assistant:second-turn",
        ],
        "continuation appended the hook's prompt as a new user message"
    );
}

#[tokio::test]
async fn on_turn_end_continuation_cap_emits_budget_limited_without_invoking_hook() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_sequence(vec![
        "t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8",
    ]));
    // Cap is 2: original turn + at most 2 continuations, then budget_limited.
    opts.turn_continuation_cap = Some(2);

    let hook_invocations = Arc::new(Mutex::new(0u32));
    let hi = hook_invocations.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let hi = hi.clone();
        Box::pin(async move {
            *hi.lock() += 1;
            TurnEndDecision::from(TurnEndAction::Continue {
                prompt: "keep going".into(),
            })
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("go").await.unwrap();

    let decisions = collect_turn_end_decisions(&received);
    let kinds: Vec<&'static str> = decisions.iter().map(|(k, _, _)| *k).collect();
    assert_eq!(
        kinds,
        vec!["continue", "continue", "budget_limited"],
        "two continues then cap-stop",
    );
    assert_eq!(
        *hook_invocations.lock(),
        2,
        "hook fires exactly twice (cap=2), then runtime stops without re-invoking",
    );
    let (_, last_count, last_reason) = decisions.last().unwrap();
    assert_eq!(*last_count, 2, "budget_limited records the final count");
    assert!(
        last_reason
            .as_ref()
            .map(|s| s.contains("continuation cap reached"))
            .unwrap_or(false),
        "budget_limited reason mentions the cap, got {:?}",
        last_reason,
    );

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert_eq!(audits.len(), 3);
    assert_eq!(audits[2]["decision"], "budget_limited");
}

// The in-harness `run_evaluator` API was removed in the multiagent rework: the goal
// evaluator now runs as the goal run's node via `multiagent::runner::run_agent`
// (tool-less judge, isolated in-memory session). Behavior is covered end-to-end in
// `crates/server/tests/goal_hook_e2e.rs` (node job registration, transcript capture,
// interrupt -> goal pause, parent-session isolation).

// ─────────────────────────────────────────────────────────────────────────────────────────
// Issue #110 sub-PR 1.5 — harness `control_plane_prompt` Custom audit emission
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Faux tool whose classifier returns `Prompt` so the agent loop routes through the
/// control-plane prompt channel, which the harness then audits.
pub(crate) struct PromptingTool {
    pub(crate) def: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl theway_core::AgentTool for PromptingTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }
    fn label(&self) -> &str {
        "prompter"
    }
    fn permission_classification(
        &self,
        _prepared_args: &serde_json::Value,
    ) -> theway_core::PermissionClassification {
        theway_core::PermissionClassification::Prompt {
            reason: "control-plane write under test".into(),
        }
    }
    async fn execute(
        &self,
        _id: &str,
        _params: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<theway_core::AgentToolUpdate>,
    ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
        Ok(theway_core::AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text(
                "did run".to_string(),
            )],
            details: serde_json::Value::Null,
            terminate: None,
        })
    }
}
