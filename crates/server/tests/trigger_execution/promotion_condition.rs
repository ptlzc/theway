//! PromotionCondition unit tests + result-details gated promotion.

use super::*;

#[test]
fn promotion_condition_any_of_returns_intersection_on_match() {
    use theway::trigger_engine::execution::PromotionCondition;

    let details = serde_json::json!({
        "dynamic_trigger": {
            "matched_rule_ids": ["dyn-keep-a", "dyn-keep-b", "dyn-other"],
        }
    });
    let condition = PromotionCondition::AnyOf {
        json_pointer: "/dynamic_trigger/matched_rule_ids".into(),
        any_of: vec!["dyn-keep-a".into(), "dyn-not-present".into()],
    };

    let matched = condition.evaluate(&details).expect("should match");
    assert_eq!(
        matched,
        vec!["dyn-keep-a".to_string()],
        "only allow-list members in the marker array intersect"
    );
}

#[test]
fn promotion_condition_any_of_fails_closed_when_pointer_missing() {
    use theway::trigger_engine::execution::{PromotionCondition, PromotionConditionSkipReason};

    // Mirrors the runtime default state before any marker tool writes through the builder.
    let details = serde_json::Value::Null;
    let condition = PromotionCondition::AnyOf {
        json_pointer: "/dynamic_trigger/matched_rule_ids".into(),
        any_of: vec!["dyn-a".into()],
    };
    assert_eq!(
        condition.evaluate(&details),
        Err(PromotionConditionSkipReason::PointerMissing),
    );
    assert_eq!(
        PromotionConditionSkipReason::PointerMissing.as_audit_str(),
        "result_details_missing",
    );
}

#[test]
fn promotion_condition_any_of_fails_closed_when_value_not_array() {
    use theway::trigger_engine::execution::{PromotionCondition, PromotionConditionSkipReason};

    let details = serde_json::json!({ "dynamic_trigger": { "matched_rule_ids": "dyn-a" } });
    let condition = PromotionCondition::AnyOf {
        json_pointer: "/dynamic_trigger/matched_rule_ids".into(),
        any_of: vec!["dyn-a".into()],
    };
    // Even if the scalar value would substring-match, it MUST NOT promote — contract is
    // "value is an array of IDs that intersect any_of," not free-form text matching.
    assert_eq!(
        condition.evaluate(&details),
        Err(PromotionConditionSkipReason::ValueNotArray),
    );
    assert_eq!(
        PromotionConditionSkipReason::ValueNotArray.as_audit_str(),
        "result_details_not_array",
    );
}

#[test]
fn promotion_condition_any_of_fails_closed_when_empty_intersection() {
    use theway::trigger_engine::execution::{PromotionCondition, PromotionConditionSkipReason};

    let details = serde_json::json!({
        "dynamic_trigger": {
            "matched_rule_ids": ["dyn-other-a", "dyn-other-b"],
        }
    });
    let condition = PromotionCondition::AnyOf {
        json_pointer: "/dynamic_trigger/matched_rule_ids".into(),
        any_of: vec!["dyn-keep".into()],
    };
    assert_eq!(
        condition.evaluate(&details),
        Err(PromotionConditionSkipReason::EmptyIntersection),
    );
    assert_eq!(
        PromotionConditionSkipReason::EmptyIntersection.as_audit_str(),
        "no_matching_rule_id",
    );
}

/// Authorization separation invariant: even if `summary` text contains the configured
/// rule IDs, promotion does NOT fire when `details` is empty. Pins the contract that
/// `summary` is display-only and never an authorization channel.
#[tokio::test]
async fn promote_when_result_details_match_does_not_consult_summary() {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, PromoteAction, PromotionCondition,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("matched dyn-promote-me explicitly"));
    before_trigger_action = Some({
        let hook: theway::trigger_engine::execution::BeforeTriggerActionHook =
            Arc::new(move |ctx: BeforeTriggerActionContext, _cancel| {
                Box::pin(async move {
                    theway::trigger_engine::execution::TriggerAction {
                        prompt: format!(
                            "{} fired: {}",
                            ctx.trigger.source_label, ctx.trigger.event_label
                        ),
                        promote: PromoteAction::PromoteSummaryWhenResultDetailsMatch {
                            template_body: None,
                            condition: PromotionCondition::AnyOf {
                                json_pointer: "/dynamic_trigger/matched_rule_ids".into(),
                                any_of: vec!["dyn-promote-me".into()],
                            },
                        },
                        promote_requires_approval: false,
                        delivery: theway::trigger_engine::execution::TriggerDelivery::SubAgent,
                    }
                })
            });
        hook
    });
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
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-struct", "trace-struct"))
        .await;
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-struct" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("must complete");

    let entries = session.entries().await.unwrap();

    // 1. No parent Message inserted — summary text alone MUST NOT authorize promotion.
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "summary substring is not an authorization channel; structured details required",
    );

    // 2. A trigger_promotion audit recorded the skip with a stable reason ID.
    let skipped = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("skipped promotion must still audit");
    assert_eq!(skipped["state"], "skipped");
    assert_eq!(skipped["reason"], "result_details_missing");
    assert_eq!(
        skipped["promote_kind"], "promote_summary_when_result_details_match",
        "audit must identify the structured-promote path"
    );

    // 3. TriggerCompleted event reports details as null (no marker tool wired yet).
    let evs = events.lock().unwrap().clone();
    let completed = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerCompleted {
                trace_id, details, ..
            } if trace_id == "trace-struct" => Some(details.clone()),
            _ => None,
        })
        .expect("TriggerCompleted");
    assert_eq!(
        completed,
        serde_json::Value::Null,
        "details defaults to null until a marker tool writes through the builder",
    );
}
