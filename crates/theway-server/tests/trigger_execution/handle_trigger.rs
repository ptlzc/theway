//! Envelope handling — accept / dedup / cycle suppression / persistence failure, runtime
//! counters, and notification-hook registration (RFC 1 sub-PR 3).

use super::*;

#[tokio::test]
async fn handle_trigger_accept_persists_audit_custom_entry_with_accepted_state() {
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

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev: TriggerEvent| {
        sink.lock().unwrap().push(ev);
    }));

    let outcome = executor
        .handle_trigger(sample_trigger("k-accept", "trace-accept"))
        .await;
    assert!(matches!(
        outcome,
        theway::trigger_engine::runtime::EvaluationOutcome::Accept
    ));

    // One Custom { custom_type: "trigger" } entry in the session.
    let entries = session.entries().await.unwrap();
    let trigger_entries: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                Some(data.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        trigger_entries.len(),
        1,
        "must persist exactly one trigger audit entry"
    );
    let data = trigger_entries[0]
        .as_ref()
        .expect("audit entry must carry data payload");
    let record: theway::trigger_engine::types::TriggerRecord =
        serde_json::from_value(data.clone()).expect("audit payload must decode as TriggerRecord");
    assert_eq!(
        record.state,
        theway::trigger_engine::types::TriggerState::Accepted
    );
    assert_eq!(record.idempotency_key, "k-accept");
    assert_eq!(record.trace_id, "trace-accept");
    assert_eq!(
        record
            .evaluator_decision
            .as_ref()
            .and_then(|v| v.get("outcome"))
            .and_then(|v| v.as_str()),
        Some("accept")
    );

    let evs = events.lock().unwrap().clone();
    let started = evs.iter().any(|e| matches!(e, TriggerEvent::TriggerHandlingStart { idempotency_key, .. } if idempotency_key == "k-accept"));
    assert!(started, "must emit TriggerHandlingStart");
    let handled = evs.iter().find_map(|e| match e {
        TriggerEvent::TriggerHandled {
            idempotency_key,
            state,
            audit_entry_id,
            ..
        } if idempotency_key == "k-accept" => Some((*state, audit_entry_id.clone())),
        _ => None,
    });
    let (state, audit_id) = handled.expect("must emit TriggerHandled for k-accept");
    assert_eq!(state, theway::trigger_engine::types::TriggerState::Accepted);
    assert!(
        audit_id.is_some(),
        "audit_entry_id must be Some on successful write"
    );
}

#[tokio::test]
async fn handle_trigger_dedup_emits_deduped_state_and_persists_record() {
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
        .handle_trigger(sample_trigger("k-dup", "trace-first"))
        .await;
    let second = executor
        .handle_trigger(sample_trigger("k-dup", "trace-second"))
        .await;
    let prev_trace_id = match second {
        theway::trigger_engine::runtime::EvaluationOutcome::Deduped {
            previous_trace_id, ..
        } => previous_trace_id,
        other => panic!("expected Deduped, got {other:?}"),
    };
    assert_eq!(prev_trace_id, "trace-first");

    let entries = session.entries().await.unwrap();
    let states: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r.state)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![
            theway::trigger_engine::types::TriggerState::Accepted,
            theway::trigger_engine::types::TriggerState::Deduped
        ],
        "must persist both audit entries in order"
    );
}

#[tokio::test]
async fn handle_trigger_cycle_suppression_persists_cycle_suppressed_state() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig {
        dedup_window: std::time::Duration::from_secs(300),
        cycle_hop_limit: 1,
    };
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
        .handle_trigger(sample_trigger("k1", "trace-loop"))
        .await;
    // Same trace at limit → suppressed.
    let suppressed = executor
        .handle_trigger(sample_trigger("k2", "trace-loop"))
        .await;
    assert!(matches!(
        suppressed,
        theway::trigger_engine::runtime::EvaluationOutcome::CycleSuppressed { .. }
    ));

    let entries = session.entries().await.unwrap();
    let last_state = entries
        .iter()
        .rev()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r.state)
            }
            _ => None,
        })
        .expect("must have at least one trigger audit entry");
    assert_eq!(
        last_state,
        theway::trigger_engine::types::TriggerState::CycleSuppressed
    );
}

#[tokio::test]
async fn notification_status_snapshot_reflects_trigger_runtime_counters() {
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

    // Fresh harness: no hooks, zero counters.
    let snap0 = executor.notification_status_snapshot();
    assert!(snap0.hooks.is_empty(), "no hooks registered yet");
    assert_eq!(snap0.runtime.accepted_total, 0);
    assert_eq!(snap0.runtime.deduped_total, 0);
    assert_eq!(snap0.runtime.cycle_suppressed_total, 0);

    let _ = executor
        .handle_trigger(sample_trigger("k1", "trace-1"))
        .await;
    let _ = executor
        .handle_trigger(sample_trigger("k2", "trace-2"))
        .await;
    let _ = executor
        .handle_trigger(sample_trigger("k1", "trace-3"))
        .await;

    let snap1 = executor.notification_status_snapshot();
    assert_eq!(snap1.runtime.accepted_total, 2);
    assert_eq!(snap1.runtime.deduped_total, 1);
    assert_eq!(snap1.runtime.cycle_suppressed_total, 0);
    assert!(snap1.runtime.dedup_entries >= 2);
}

#[tokio::test]
async fn handle_trigger_persistence_failure_still_returns_outcome_and_emits_error() {
    use async_trait::async_trait;
    use std::sync::Arc;

    /// Storage that fails every `append_entry` to verify the audit-failure reflux path.
    struct FailingAppendStorage {
        inner: Arc<MemorySessionStorage>,
    }

    #[async_trait]
    impl SessionStorage for FailingAppendStorage {
        async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
            self.inner.get_metadata_json().await
        }
        async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
            Err(SessionError {
                code: SessionErrorCode::StorageFailure,
                message: "synthetic write failure".into(),
            })
        }
        async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
            self.inner.get_entry(id).await
        }
        async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
            self.inner.get_entries().await
        }
        async fn get_path_to_root(
            &self,
            entry_id: Option<&str>,
        ) -> Result<Vec<SessionTreeEntry>, SessionError> {
            self.inner.get_path_to_root(entry_id).await
        }
        async fn find_entries(
            &self,
            entry_type: &str,
        ) -> Result<Vec<SessionTreeEntry>, SessionError> {
            self.inner.find_entries(entry_type).await
        }
        async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
            self.inner.get_leaf_id().await
        }
        async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
            self.inner.set_leaf_id(id).await
        }
        async fn create_entry_id(&self) -> Result<String, SessionError> {
            self.inner.create_entry_id().await
        }
        async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
            self.inner.get_label(id).await
        }
    }

    let storage = Arc::new(FailingAppendStorage {
        inner: Arc::new(MemorySessionStorage::new()),
    });
    let session = Session::new(storage as Arc<dyn SessionStorage>);
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

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev: TriggerEvent| {
        sink.lock().unwrap().push(ev);
    }));

    let outcome = executor
        .handle_trigger(sample_trigger("k-persist-fail", "trace-x"))
        .await;
    assert!(
        matches!(
            outcome,
            theway::trigger_engine::runtime::EvaluationOutcome::Accept
        ),
        "evaluator outcome must be authoritative even when audit persistence fails"
    );

    let evs = events.lock().unwrap().clone();
    let saw_persist_err = evs.iter().any(|e| {
        matches!(
            e,
            TriggerEvent::PersistenceError { context, .. } if context == "trigger_audit"
        )
    });
    assert!(
        saw_persist_err,
        "must emit PersistenceError on audit write failure"
    );
    let handled_audit_id = evs.iter().find_map(|e| match e {
        TriggerEvent::TriggerHandled { audit_entry_id, .. } => Some(audit_entry_id.clone()),
        _ => None,
    });
    assert!(
        handled_audit_id.is_some() && handled_audit_id.as_ref().unwrap().is_none(),
        "TriggerHandled.audit_entry_id must be None when persistence failed"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// register_notification_hook — RFC 1 sub-PR 3 (hook supervisor)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn register_notification_hook_drives_pump_into_handle_trigger() {
    use theway::trigger_engine::notification_hook::{
        DynNotificationHook, HookError, HookState, NotificationHook, NotificationHookStatus,
        TriggerSink,
    };

    /// Mock hook: pushes a fixed number of triggers and then closes the sink so the pump
    /// exits cleanly. Verifies that the harness's supervisor actually drives `run(sink)`
    /// and routes everything to `handle_trigger`.
    struct CountedHook {
        label: String,
        triggers: std::sync::Mutex<Vec<theway::trigger_engine::types::Trigger>>,
    }
    #[async_trait::async_trait]
    impl NotificationHook for CountedHook {
        fn label(&self) -> &str {
            &self.label
        }
        async fn run(&self, sink: TriggerSink) -> Result<(), HookError> {
            let triggers: Vec<_> = self.triggers.lock().unwrap().drain(..).collect();
            for t in triggers {
                sink.send(t).map_err(|_| HookError::SinkClosed)?;
            }
            Ok(())
        }
        fn status(&self) -> NotificationHookStatus {
            NotificationHookStatus {
                state: HookState::Connected,
                last_event_at: None,
                last_ack_at: None,
                last_error: None,
                queued_count: 0,
                dropped_count: 0,
                deduped_count: 0,
                subscription_labels: vec![self.label.clone()],
                requires_attention: None,
            }
        }
    }

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;
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

    let triggers = vec![
        sample_trigger("hook-k1", "hook-trace-1"),
        sample_trigger("hook-k2", "hook-trace-2"),
        sample_trigger("hook-k1", "hook-trace-3"), // duplicate of k1 → dedup path
    ];

    let hook: DynNotificationHook = Arc::new(CountedHook {
        label: "mock".into(),
        triggers: std::sync::Mutex::new(triggers),
    });

    executor.register_notification_hook(hook);

    // Wait for the pump to drain. The hook produces three triggers synchronously then
    // closes the sink; the pump exits when rx.recv() returns None. We poll the snapshot
    // counters as the completion signal; with a wide timeout to handle CI load.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let snap = loop {
        let s = executor.notification_status_snapshot();
        if s.runtime.accepted_total + s.runtime.deduped_total + s.runtime.cycle_suppressed_total
            >= 3
        {
            break s;
        }
        if std::time::Instant::now() > deadline {
            panic!(
                "pump did not process 3 triggers within 5s — snapshot: {:?}",
                s
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };

    assert_eq!(snap.runtime.accepted_total, 2);
    assert_eq!(snap.runtime.deduped_total, 1);
    assert_eq!(snap.runtime.cycle_suppressed_total, 0);
    assert_eq!(snap.hooks.len(), 1, "hook must be tracked in snapshot");
    assert_eq!(
        snap.hooks[0].subscription_labels,
        vec!["mock".to_string()],
        "snapshot hook label must round-trip from hook.status()"
    );

    // Both accepted triggers must have produced audit Custom entries.
    let entries = session.entries().await.unwrap();
    let trigger_audit_count = entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                SessionTreeEntry::Custom { custom_type, .. }
                    if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE
            )
        })
        .count();
    // 3 audit entries: Accepted (k1), Accepted (k2), Deduped (k1 again).
    assert_eq!(trigger_audit_count, 3);
}

#[tokio::test]
async fn register_notification_hook_snapshot_reflects_hook_status_state() {
    use theway::trigger_engine::notification_hook::{
        DynNotificationHook, HookError, HookState, NotificationHook, NotificationHookStatus,
        TriggerSink,
    };

    /// Hook that immediately reports `Disconnected` and never sends anything. The supervisor
    /// pump exits as soon as the hook's `run` future resolves and the sink is dropped.
    struct DegradedHook;
    #[async_trait::async_trait]
    impl NotificationHook for DegradedHook {
        fn label(&self) -> &str {
            "degraded"
        }
        async fn run(&self, _sink: TriggerSink) -> Result<(), HookError> {
            Ok(())
        }
        fn status(&self) -> NotificationHookStatus {
            NotificationHookStatus {
                state: HookState::Disconnected {
                    reason: "transport closed at startup".into(),
                },
                last_event_at: None,
                last_ack_at: None,
                last_error: Some("transport closed at startup".into()),
                queued_count: 0,
                dropped_count: 0,
                deduped_count: 0,
                subscription_labels: vec!["degraded".into()],
                requires_attention: Some("degraded: transport closed at startup".into()),
            }
        }
    }

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn> = None;
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

    let hook: DynNotificationHook = Arc::new(DegradedHook);
    executor.register_notification_hook(hook);

    // Give the driver/pump tasks a moment to schedule. The hook's run returns immediately
    // so we mostly need to give the snapshot a chance to see the registered hook.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let snap = executor.notification_status_snapshot();
    assert_eq!(snap.hooks.len(), 1);
    assert!(
        matches!(snap.hooks[0].state, HookState::Disconnected { .. }),
        "snapshot must reflect the hook's reported state"
    );
    assert_eq!(
        snap.hooks[0].requires_attention.as_deref(),
        Some("degraded: transport closed at startup")
    );
    // Hook produced nothing so runtime counters stay at zero.
    assert_eq!(snap.runtime.accepted_total, 0);
}
