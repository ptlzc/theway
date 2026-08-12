//! before_trigger Prompt path — payload identity binding, abort, dedup exclusion (RFC 1 sub-PR 4).

use super::*;

#[tokio::test]
async fn before_trigger_prompt_allow_admits_trigger_and_binds_source_identity() {
    use theway::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook, OnTriggerPromptHook,
        TriggerPromptDecision,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook>;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let prompt_hook: BeforeTriggerHook = Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
        Box::pin(async move {
            BeforeTriggerDecision::Prompt {
                reason: "new source sender requires approval".into(),
            }
        })
    });
    before_trigger = Some(prompt_hook);

    let seen_request = Arc::new(std::sync::Mutex::new(None));
    let seen_request_sink = seen_request.clone();
    let trigger_prompt: OnTriggerPromptHook = Arc::new(move |request, _cancel| {
        *seen_request_sink.lock().unwrap() = Some(request);
        Box::pin(async move { TriggerPromptDecision::Allow })
    });
    on_trigger_prompt = Some(trigger_prompt);
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

    let mut trigger = sample_trigger("prompt-allow", "trace-prompt-allow");
    trigger.source_kind = theway::trigger_engine::types::SourceKind::Mcp;
    trigger.source_label = "external notifier".into();
    trigger.event_label = "notification".into();
    trigger.payload_visibility = theway::trigger_engine::types::PayloadVisibility::Shared;
    trigger.payload_summary = Some("alice sent a notification".into());
    trigger.payload = Some(serde_json::json!({
        "_meta": {
            "receiver_agent_id": "11111111-1111-4111-8111-111111111111",
            "sender_agent_id": "22222222-2222-4222-8222-222222222222",
            "action_class": "notification"
        },
        "secret_body": "this raw payload must stay out of prompt preview"
    }));
    trigger.authority.principal_id = "22222222-2222-4222-8222-222222222222".into();

    let outcome = executor.handle_trigger(trigger).await;
    assert!(matches!(
        outcome,
        theway::trigger_engine::runtime::EvaluationOutcome::Accept
    ));

    let request = seen_request
        .lock()
        .unwrap()
        .clone()
        .expect("on_trigger_prompt hook must receive request");
    assert_eq!(
        request.receiver_agent_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(
        request.sender_agent_id,
        "22222222-2222-4222-8222-222222222222"
    );
    assert_eq!(request.action_class, "notification");
    assert_eq!(request.reason, "new source sender requires approval");
    assert!(
        !request.payload.to_string().contains("secret_body"),
        "prompt preview must never carry raw trigger payload"
    );

    let entries = session.entries().await.unwrap();
    let trigger_record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r)
            }
            _ => None,
        })
        .expect("trigger audit entry");
    assert_eq!(
        trigger_record.state,
        theway::trigger_engine::types::TriggerState::Accepted
    );
    let decision = trigger_record
        .evaluator_decision
        .as_ref()
        .expect("evaluator decision");
    assert_eq!(decision["permission"].as_str(), Some("prompt"));
    assert_eq!(decision["prompt_decision"].as_str(), Some("allow"));
    assert_eq!(
        decision["trigger_prompt_id"].as_str(),
        Some(request.trigger_prompt_id.as_str())
    );

    let prompt_audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_prompt" => data.clone(),
            _ => None,
        })
        .expect("trigger_prompt audit entry");
    assert_eq!(prompt_audit["decision"].as_str(), Some("allow"));
    assert_eq!(
        prompt_audit["trigger_prompt_id"].as_str(),
        Some(request.trigger_prompt_id.as_str())
    );
    assert_eq!(
        prompt_audit["receiver_agent_id"].as_str(),
        Some("11111111-1111-4111-8111-111111111111")
    );
}

#[tokio::test]
async fn before_trigger_prompt_prefers_meta_binding_over_legacy_top_level_fields() {
    use theway::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook, OnTriggerPromptHook,
        TriggerPromptDecision,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook>;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let prompt_hook: BeforeTriggerHook = Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
        Box::pin(async move {
            BeforeTriggerDecision::Prompt {
                reason: "new source sender requires approval".into(),
            }
        })
    });
    before_trigger = Some(prompt_hook);

    let seen_request = Arc::new(std::sync::Mutex::new(None));
    let seen_request_sink = seen_request.clone();
    let trigger_prompt: OnTriggerPromptHook = Arc::new(move |request, _cancel| {
        *seen_request_sink.lock().unwrap() = Some(request);
        Box::pin(async move { TriggerPromptDecision::Allow })
    });
    on_trigger_prompt = Some(trigger_prompt);
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

    let mut trigger = sample_trigger("prompt-meta-precedence", "trace-meta-precedence");
    trigger.payload = Some(serde_json::json!({
        "receiver_agent_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "sender_agent_id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "action_class": "legacy.notification",
        "_meta": {
            "receiver_agent_id": "11111111-1111-4111-8111-111111111111",
            "sender_agent_id": "22222222-2222-4222-8222-222222222222",
            "action_class": "notification"
        }
    }));

    let _ = executor.handle_trigger(trigger).await;

    let request = seen_request
        .lock()
        .unwrap()
        .clone()
        .expect("on_trigger_prompt hook must receive request");
    assert_eq!(
        request.receiver_agent_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(
        request.sender_agent_id,
        "22222222-2222-4222-8222-222222222222"
    );
    assert_eq!(request.action_class, "notification");
}

#[tokio::test]
async fn before_trigger_prompt_rejects_untrusted_payload_identity_fields_and_caps_reasons() {
    use theway::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook, OnTriggerPromptHook,
        TriggerPromptDecision,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook>;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let oversized_prompt_reason = format!("prompt-reason-{}", "x".repeat(700));
    let prompt_reason_for_hook = oversized_prompt_reason.clone();
    let prompt_hook: BeforeTriggerHook = Arc::new(move |_ctx: BeforeTriggerContext, _cancel| {
        let prompt_reason_for_hook = prompt_reason_for_hook.clone();
        Box::pin(async move {
            BeforeTriggerDecision::Prompt {
                reason: prompt_reason_for_hook,
            }
        })
    });
    before_trigger = Some(prompt_hook);

    let oversized_deny_reason = format!("deny-reason-{}", "y".repeat(700));
    let deny_reason_for_hook = oversized_deny_reason.clone();
    let trigger_prompt: OnTriggerPromptHook = Arc::new(move |_request, _cancel| {
        let deny_reason_for_hook = deny_reason_for_hook.clone();
        Box::pin(async move {
            TriggerPromptDecision::Deny {
                reason: Some(deny_reason_for_hook),
            }
        })
    });
    on_trigger_prompt = Some(trigger_prompt);
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

    let mut trigger = sample_trigger("prompt-invalid-meta", "trace-invalid-meta");
    trigger.source_kind = theway::trigger_engine::types::SourceKind::Mcp;
    trigger.event_label = "notification".into();
    trigger.payload = Some(serde_json::json!({
        "_meta": {
            "receiver_agent_id": "sk-receiver-secret-token",
            "sender_agent_id": "Bearer sender-secret-token",
            "action_class": "sk-action-secret-token"
        }
    }));
    trigger.authority.principal_id = "33333333-3333-4333-8333-333333333333".into();

    let _ = executor.handle_trigger(trigger).await;

    let evs = events.lock().unwrap().clone();
    let request = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerPromptRequest { request } => Some(request.clone()),
            _ => None,
        })
        .expect("Prompt decision must emit a trigger prompt request");
    assert_eq!(request.receiver_agent_id, None);
    assert_eq!(
        request.sender_agent_id,
        "33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(request.action_class, "notification");
    assert!(
        request.reason.chars().count() <= 512,
        "request reason must be bounded"
    );
    assert!(
        request.reason.ends_with('…'),
        "oversized request reason should carry truncation marker"
    );
    let request_string = serde_json::to_string(&request.payload).unwrap();
    assert!(!request_string.contains("sk-receiver-secret-token"));
    assert!(!request_string.contains("Bearer sender-secret-token"));
    assert!(!request_string.contains("sk-action-secret-token"));

    let entries = session.entries().await.unwrap();
    let prompt_audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_prompt" => data.clone(),
            _ => None,
        })
        .expect("trigger_prompt audit entry");
    assert_eq!(prompt_audit["receiver_agent_id"], serde_json::Value::Null);
    assert_eq!(
        prompt_audit["sender_agent_id"].as_str(),
        Some("33333333-3333-4333-8333-333333333333")
    );
    assert_eq!(prompt_audit["action_class"].as_str(), Some("notification"));
    let audit_reason = prompt_audit["reason"].as_str().unwrap();
    assert!(audit_reason.chars().count() <= 512);
    assert!(audit_reason.ends_with('…'));
    let audit_string = serde_json::to_string(&prompt_audit).unwrap();
    assert!(!audit_string.contains("sk-receiver-secret-token"));
    assert!(!audit_string.contains("Bearer sender-secret-token"));
    assert!(!audit_string.contains("sk-action-secret-token"));

    let trigger_record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r)
            }
            _ => None,
        })
        .expect("trigger audit entry");
    let decision = trigger_record
        .evaluator_decision
        .as_ref()
        .expect("evaluator decision");
    assert!(decision["reason"].as_str().unwrap().chars().count() <= 512);
    assert!(
        decision["decision_reason"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            <= 512
    );
}

#[tokio::test]
async fn before_trigger_prompt_abort_cancels_in_flight_prompt_hook() {
    use theway::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook, OnTriggerPromptHook,
        TriggerPromptDecision,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook>;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let prompt_hook: BeforeTriggerHook = Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
        Box::pin(async move {
            BeforeTriggerDecision::Prompt {
                reason: "needs approval".into(),
            }
        })
    });
    before_trigger = Some(prompt_hook);

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let trigger_prompt: OnTriggerPromptHook = Arc::new(move |_request, cancel| {
        let started_tx = started_tx.clone();
        Box::pin(async move {
            if let Some(tx) = started_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            cancel.cancelled().await;
            TriggerPromptDecision::Timeout { reason: None }
        })
    });
    on_trigger_prompt = Some(trigger_prompt);

    let harness = Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session.clone(),
    )));
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
    let run_executor = executor.clone();
    let join = tokio::spawn(async move {
        run_executor
            .handle_trigger(sample_trigger("prompt-abort", "trace-prompt-abort"))
            .await
    });

    started_rx.await.expect("prompt hook should start");
    executor.abort();
    let outcome = join.await.expect("trigger handling task should finish");
    assert!(matches!(
        outcome,
        theway::trigger_engine::runtime::EvaluationOutcome::Accept
    ));

    let entries = session.entries().await.unwrap();
    let trigger_record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r)
            }
            _ => None,
        })
        .expect("trigger audit entry");
    assert_eq!(
        trigger_record.state,
        theway::trigger_engine::types::TriggerState::NeedsApproval
    );
    assert_eq!(
        trigger_record.evaluator_decision.as_ref().unwrap()["prompt_decision"].as_str(),
        Some("timeout")
    );

    let prompt_audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_prompt" => data.clone(),
            _ => None,
        })
        .expect("trigger_prompt audit entry");
    assert_eq!(prompt_audit["decision"].as_str(), Some("timeout"));
    assert_eq!(prompt_audit["reason"].as_str(), None);
}

#[tokio::test]
async fn before_trigger_hook_does_not_run_on_deduped_path() {
    use theway::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook>;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;

    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = call_count.clone();
    let hook: BeforeTriggerHook = Arc::new(move |_ctx: BeforeTriggerContext, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            BeforeTriggerDecision::Allow
        })
    });
    before_trigger = Some(hook);
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

    // First call: Accept → hook runs once.
    let _ = executor
        .handle_trigger(sample_trigger("dup-key", "trace-1"))
        .await;
    // Second call (duplicate idempotency key): Deduped → hook MUST NOT run.
    let _ = executor
        .handle_trigger(sample_trigger("dup-key", "trace-2"))
        .await;

    assert_eq!(
        call_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "hook must only run after evaluator Accept, never on Deduped/CycleSuppressed paths"
    );
}
