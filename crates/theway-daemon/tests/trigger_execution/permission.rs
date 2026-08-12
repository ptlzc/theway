//! before_trigger permission evaluator — default allow / deny / prompt approval flows (RFC 1 sub-PR 4).

use super::*;

// ─────────────────────────────────────────────────────────────────────────────────────────
// before_trigger hook — RFC 1 sub-PR 4 (permission evaluator extension)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn before_trigger_default_allow_keeps_state_accepted() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let _ = executor
        .handle_trigger(sample_trigger("perm-default", "trace-default"))
        .await;

    let entries = session.entries().await.unwrap();
    let state = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type
                == theway_daemon::trigger_engine::types::TriggerRecord::CUSTOM_TYPE =>
            {
                let r: theway_daemon::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r.state)
            }
            _ => None,
        })
        .expect("audit entry");
    assert_eq!(
        state,
        theway_daemon::trigger_engine::types::TriggerState::Accepted,
        "no hook → default Allow → Accepted"
    );
}

#[tokio::test]
async fn before_trigger_deny_records_permission_denied_state_and_reason() {
    use theway_daemon::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let deny_hook: BeforeTriggerHook = Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
        Box::pin(async move {
            BeforeTriggerDecision::Deny {
                reason: "principal not on allow-list".into(),
            }
        })
    });
    before_trigger = Some(deny_hook);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev: TriggerEvent| {
        sink.lock().unwrap().push(ev);
    }));

    let outcome = executor
        .handle_trigger(sample_trigger("perm-deny", "trace-deny"))
        .await;
    assert!(
        matches!(
            outcome,
            theway_daemon::trigger_engine::runtime::EvaluationOutcome::Accept
        ),
        "EvaluationOutcome is still Accept (evaluator decided to admit); the harness state is what reflects the deny"
    );

    let entries = session.entries().await.unwrap();
    let record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type
                == theway_daemon::trigger_engine::types::TriggerRecord::CUSTOM_TYPE =>
            {
                let r: theway_daemon::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r)
            }
            _ => None,
        })
        .expect("audit entry");

    assert_eq!(
        record.state,
        theway_daemon::trigger_engine::types::TriggerState::PermissionDenied
    );
    let decision = record
        .evaluator_decision
        .as_ref()
        .expect("evaluator_decision must capture deny reason");
    assert_eq!(decision["permission"].as_str(), Some("deny"));
    assert_eq!(
        decision["reason"].as_str(),
        Some("principal not on allow-list")
    );

    // The live event must carry the same evaluator_decision the audit got, so TUI / JSONL
    // subscribers can render the deny reason without re-reading the session.
    let evs = events.lock().unwrap().clone();
    let event_decision = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerHandled {
                state,
                evaluator_decision,
                ..
            } if *state == theway_daemon::trigger_engine::types::TriggerState::PermissionDenied => {
                Some(evaluator_decision.clone())
            }
            _ => None,
        })
        .expect("TriggerHandled event with PermissionDenied state must exist");
    let event_decision = event_decision.expect("event must carry evaluator_decision");
    assert_eq!(event_decision["permission"].as_str(), Some("deny"));
    assert_eq!(
        event_decision["reason"].as_str(),
        Some("principal not on allow-list")
    );
}

#[tokio::test]
async fn before_trigger_prompt_records_needs_approval_state_and_reason() {
    use theway_daemon::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let prompt_hook: BeforeTriggerHook = Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
        Box::pin(async move {
            BeforeTriggerDecision::Prompt {
                reason: "external trigger from new principal".into(),
            }
        })
    });
    before_trigger = Some(prompt_hook);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev: TriggerEvent| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("perm-prompt", "trace-prompt"))
        .await;

    let entries = session.entries().await.unwrap();
    let record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type
                == theway_daemon::trigger_engine::types::TriggerRecord::CUSTOM_TYPE =>
            {
                let r: theway_daemon::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r)
            }
            _ => None,
        })
        .expect("audit entry");

    assert_eq!(
        record.state,
        theway_daemon::trigger_engine::types::TriggerState::NeedsApproval
    );
    assert_eq!(
        record.evaluator_decision.as_ref().unwrap()["permission"].as_str(),
        Some("prompt")
    );

    let evs = events.lock().unwrap().clone();
    let (handled_state, handled_decision) = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerHandled {
                state,
                evaluator_decision,
                ..
            } => Some((*state, evaluator_decision.clone())),
            _ => None,
        })
        .expect("must emit TriggerHandled");
    assert_eq!(
        handled_state,
        theway_daemon::trigger_engine::types::TriggerState::NeedsApproval,
        "TriggerHandled event must carry the policy-terminal state"
    );
    // Live subscribers (TUI banner, JSONL logs) must be able to render the prompt reason
    // straight from the event without a secondary session lookup.
    let decision = handled_decision.expect("TriggerHandled must carry evaluator_decision");
    assert_eq!(decision["permission"].as_str(), Some("prompt"));
    assert_eq!(
        decision["reason"].as_str(),
        Some("external trigger from new principal")
    );

    let prompt_event = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerPromptRequest { request } => Some(request.clone()),
            _ => None,
        })
        .expect("Prompt decision must emit a trigger prompt request");
    assert_eq!(prompt_event.trace_id, "trace-prompt");
    assert_eq!(prompt_event.sender_agent_id, "mcp:github");
    assert_eq!(prompt_event.action_class, "pr merged");
    assert!(
        prompt_event.payload.get("payload").is_none(),
        "prompt preview must not include raw trigger payload"
    );

    let prompt_audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_prompt" => data.clone(),
            _ => None,
        })
        .expect("trigger_prompt audit entry must be written");
    assert_eq!(prompt_audit["decision"].as_str(), Some("deny"));
    assert_eq!(
        prompt_audit["reason"].as_str(),
        Some(
            "trigger prompt required but no on_trigger_prompt hook configured \
             (fail-closed deny — see issue #110 design v0.2)"
        )
    );
    assert_eq!(
        prompt_audit["trigger_prompt_id"].as_str(),
        Some(prompt_event.trigger_prompt_id.as_str())
    );
}
