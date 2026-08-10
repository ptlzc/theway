//! Integration tests for the CLI trigger engine (`theway::trigger_engine`), ported from the
//! core harness e2e suite when the trigger pipeline moved out of theway-core. Covers the
//! envelope → dedup/cycle → permission → audit → sub-agent → promotion pipeline through
//! the public `TriggerExecutor` API (host-side counterpart of the old
//! `AgentHarness::handle_trigger`).

use std::sync::Arc;

use theway::trigger_engine::event::TriggerEvent;
use theway::trigger_engine::execution::{
    BeforeTriggerActionHook, BeforeTriggerHook, OnTriggerPromptHook, TriggerExecutor,
};
use theway::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session, SessionError,
    SessionErrorCode, SessionStorage, SessionTreeEntry, StreamFn,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream_fn(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
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

fn sample_trigger(idempotency_key: &str, trace_id: &str) -> theway::trigger_engine::types::Trigger {
    theway::trigger_engine::types::Trigger {
        source: theway::trigger_engine::types::TriggerSource::Mcp {
            server_name: "github".into(),
            method: "notifications/pr.merged".into(),
        },
        source_kind: theway::trigger_engine::types::SourceKind::Mcp,
        source_label: "MCP github".into(),
        event_label: "pr merged".into(),
        payload_visibility: theway::trigger_engine::types::PayloadVisibility::Local,
        payload_summary: Some("PR #42 merged".into()),
        payload: None,
        idempotency_key: idempotency_key.into(),
        replacement_policy: theway::trigger_engine::types::ReplacementPolicy::Drop,
        trace_id: trace_id.into(),
        authority: theway::trigger_engine::types::TriggerAuthority {
            principal_id: "mcp:github".into(),
            principal_label: "github".into(),
            credential_scope: theway::trigger_engine::types::CredentialScope::Project,
            allowed_source_actions: vec!["read".into()],
            expires_at: None,
        },
        received_at: chrono::Utc::now(),
    }
}

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
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r.state)
            }
            _ => None,
        })
        .expect("audit entry");
    assert_eq!(
        state,
        theway::trigger_engine::types::TriggerState::Accepted,
        "no hook → default Allow → Accepted"
    );
}

#[tokio::test]
async fn before_trigger_deny_records_permission_denied_state_and_reason() {
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
            theway::trigger_engine::runtime::EvaluationOutcome::Accept
        ),
        "EvaluationOutcome is still Accept (evaluator decided to admit); the harness state is what reflects the deny"
    );

    let entries = session.entries().await.unwrap();
    let record = entries
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
        .expect("audit entry");

    assert_eq!(
        record.state,
        theway::trigger_engine::types::TriggerState::PermissionDenied
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
            } if *state == theway::trigger_engine::types::TriggerState::PermissionDenied => {
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
            } if custom_type == theway::trigger_engine::types::TriggerRecord::CUSTOM_TYPE => {
                let r: theway::trigger_engine::types::TriggerRecord =
                    serde_json::from_value(data.as_ref().unwrap().clone()).unwrap();
                Some(r)
            }
            _ => None,
        })
        .expect("audit entry");

    assert_eq!(
        record.state,
        theway::trigger_engine::types::TriggerState::NeedsApproval
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
        theway::trigger_engine::types::TriggerState::NeedsApproval,
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

// ─────────────────────────────────────────────────────────────────────────────────────────
// Sub-agent execution — RFC 1 sub-PR 5a (Accepted → Running → Completed/Failed)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Helper: wait until a predicate over the captured event log returns Some(value) or the
/// deadline elapses. Polls every 20ms.
async fn wait_for_event<F, T>(
    events: &Arc<std::sync::Mutex<Vec<TriggerEvent>>>,
    timeout_secs: u64,
    mut pred: F,
) -> Option<T>
where
    F: FnMut(&[TriggerEvent]) -> Option<T>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Some(v) = pred(&events.lock().unwrap()) {
            return Some(v);
        }
        if std::time::Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn accepted_trigger_spawns_sub_agent_and_writes_trigger_result_audit() {
    // Faux model returns one assistant message and stops; sub-agent runs cleanly.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("sub-agent done"));
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
        .handle_trigger(sample_trigger("k-spawn", "trace-spawn"))
        .await;

    // Wait for TriggerCompleted.
    let completed = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted {
                trace_id, summary, ..
            } if trace_id == "trace-spawn" => Some(summary.clone()),
            _ => None,
        })
    })
    .await;
    assert!(completed.is_some(), "must emit TriggerCompleted");

    // trigger_result audit must exist with success=true, trace_id link, summary from
    // sub-agent's assistant message.
    let entries = session.entries().await.unwrap();
    let record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => Some(data.clone()),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    let data = record.expect("trigger_result must carry data");
    assert_eq!(data["trace_id"].as_str(), Some("trace-spawn"));
    assert_eq!(data["success"].as_bool(), Some(true));
    assert_eq!(
        data["summary"].as_str(),
        Some("sub-agent done"),
        "summary must be the sub-agent's final assistant text"
    );
    assert!(data["branch_id"].is_null(), "5a in-memory: branch_id null");
}

#[tokio::test]
async fn event_ordering_handled_then_started_then_completed() {
    // RFC 1 §5.F: HarnessHandled(Accepted) → TriggerExecutionStarted → TriggerCompleted
    // must always be observed in that order for the same trace_id.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("ok"));
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
        .handle_trigger(sample_trigger("k-order", "trace-order"))
        .await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-order" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("must complete");

    // Now find indices of the three events for trace-order.
    let evs = events.lock().unwrap().clone();
    let mut handled_idx = None;
    let mut started_idx = None;
    let mut completed_idx = None;
    for (i, e) in evs.iter().enumerate() {
        match e {
            TriggerEvent::TriggerHandled {
                trace_id,
                state: theway::trigger_engine::types::TriggerState::Accepted,
                ..
            } if trace_id == "trace-order" => handled_idx = Some(i),
            TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-order" => {
                started_idx = Some(i)
            }
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-order" => {
                completed_idx = Some(i)
            }
            _ => {}
        }
    }
    let h = handled_idx.expect("TriggerHandled(Accepted)");
    let s = started_idx.expect("TriggerExecutionStarted");
    let c = completed_idx.expect("TriggerCompleted");
    assert!(
        h < s && s < c,
        "expected Handled({h}) < Started({s}) < Completed({c}); events={:?}",
        evs.iter().map(std::mem::discriminant).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn pump_non_blocking_second_trigger_audited_while_first_runs() {
    // RFC 1 §5.G acceptance #1: even if the first trigger's sub-agent is slow, the second
    // trigger reaches handle_trigger's audit/`TriggerHandled` event promptly.
    // We simulate "slow first" by making the faux stream sleep before emitting Done.
    use tokio::sync::Mutex as TokioMutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Slow stream: holds Done event for 500ms.
    let stream_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stream_count_for_fn = stream_count.clone();
    let stream_fn_val: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let nth = stream_count_for_fn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            // Only the FIRST sub-agent's stream is slow; subsequent (here, the second
            // trigger's sub-agent) returns immediately.
            if nth == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
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
    });
    stream_fn = Some(stream_fn_val);
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

    // Fire trigger 1 — sub-agent will run slowly in the background.
    let _ = TokioMutex::new(()); // silence import warning if unused
    let t0 = std::time::Instant::now();
    let _ = executor
        .handle_trigger(sample_trigger("k-slow", "trace-slow"))
        .await;
    let elapsed_1 = t0.elapsed();
    // handle_trigger returns promptly even though sub-agent will be slow.
    assert!(
        elapsed_1 < std::time::Duration::from_millis(200),
        "handle_trigger must return promptly; took {elapsed_1:?}"
    );

    // Fire trigger 2 right after. It should reach TriggerHandled audit within a few ms,
    // unaffected by trigger 1's slow sub-agent.
    let t1 = std::time::Instant::now();
    let _ = executor
        .handle_trigger(sample_trigger("k-fast", "trace-fast"))
        .await;
    let elapsed_2 = t1.elapsed();
    assert!(
        elapsed_2 < std::time::Duration::from_millis(200),
        "second handle_trigger must not block on first sub-agent; took {elapsed_2:?}"
    );

    wait_for_event(&events, 2, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerHandled { trace_id, .. } if trace_id == "trace-fast" => Some(()),
            _ => None,
        })
    })
    .await
    .expect("second trigger must reach TriggerHandled within 2s");
}

#[tokio::test]
async fn running_snapshot_lists_in_flight_trigger_with_preview() {
    // RFC 1 §5.G: notification_status_snapshot().running shows in-flight trigger with
    // preview-safe bounded fields.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Slow stream so the trigger stays in-flight long enough to inspect.
    let stream_fn_val: StreamFn = Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
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
    });
    stream_fn = Some(stream_fn_val);
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
        .handle_trigger(sample_trigger("k-running", "trace-running"))
        .await;

    // Poll snapshot for the in-flight entry.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut found = None;
    while std::time::Instant::now() < deadline {
        let snap = executor.notification_status_snapshot();
        if let Some(rt) = snap.running.iter().find(|r| r.trace_id == "trace-running") {
            found = Some(rt.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let rt = found.expect("running snapshot must include in-flight trigger");
    assert_eq!(rt.source_label, "MCP github");
    assert_eq!(rt.event_label, "pr merged");
    // Default action prompt: "MCP github fired: pr merged"
    assert!(
        rt.prompt_preview.contains("MCP github") && rt.prompt_preview.contains("pr merged"),
        "preview must reflect default prompt mapping, got {:?}",
        rt.prompt_preview
    );

    // After completion the entry leaves the snapshot.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let snap_after = executor.notification_status_snapshot();
    assert!(
        snap_after
            .running
            .iter()
            .all(|r| r.trace_id != "trace-running"),
        "running snapshot must drop completed triggers"
    );
}

#[tokio::test]
async fn abort_trigger_cancels_in_flight_sub_agent_and_emits_failed() {
    // RFC 1 §5.G acceptance #7: abort_trigger while sub-agent mid-execution → TriggerFailed
    // within 1s, trigger_result.summary == "aborted" (in our impl: status preserved + reason
    // "aborted" carried via TriggerFailed.reason).
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Very slow stream so we can abort mid-flight.
    let stream_fn_val: StreamFn = Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
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
    });
    stream_fn = Some(stream_fn_val);
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
        .handle_trigger(sample_trigger("k-abort", "trace-abort"))
        .await;

    // Let TriggerExecutionStarted land first so we know the task is running.
    wait_for_event(&events, 2, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-abort" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("ExecutionStarted must fire before abort");

    executor.abort_trigger("trace-abort");

    // TriggerFailed must arrive within ~1s (allow 3s for CI slack).
    let reason = wait_for_event(&events, 3, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerFailed { trace_id, reason } if trace_id == "trace-abort" => {
                Some(reason.clone())
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerFailed must arrive within 3s of abort");
    assert_eq!(
        reason, "aborted",
        "abort_trigger must emit TriggerFailed with reason \"aborted\""
    );

    // trigger_result audit reflects the failure.
    let entries = session.entries().await.unwrap();
    let record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => Some(data.clone()),
            _ => None,
        })
        .expect("trigger_result must be written even on abort");
    let data = record.expect("data");
    assert_eq!(data["success"].as_bool(), Some(false));
    assert_eq!(data["trace_id"].as_str(), Some("trace-abort"));
}

#[tokio::test]
async fn non_accepted_states_do_not_spawn_sub_agent() {
    // Dedup/CycleSuppressed/PermissionDenied must NOT trigger sub-agent execution. Verify
    // by sending a duplicate trigger and confirming no TriggerExecutionStarted fires for
    // it.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("done"));
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

    // First admission → spawn (Accepted).
    let _ = executor
        .handle_trigger(sample_trigger("k-dedup-test", "trace-1"))
        .await;
    // Second admission with same key → Deduped, NO spawn.
    let _ = executor
        .handle_trigger(sample_trigger("k-dedup-test", "trace-2"))
        .await;

    // Wait for the FIRST trigger to finish so all expected events are present.
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-1" => Some(()),
            _ => None,
        })
    })
    .await
    .expect("first trigger completes");

    // Now scan for any TriggerExecutionStarted with trace_id == "trace-2".
    let evs = events.lock().unwrap().clone();
    let spawned_for_trace_2 = evs.iter().any(|e| {
        matches!(
            e,
            TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-2"
        )
    });
    assert!(
        !spawned_for_trace_2,
        "Deduped trigger must NOT spawn a sub-agent; got events: {:?}",
        evs.iter()
            .filter_map(|e| match e {
                TriggerEvent::TriggerExecutionStarted { trace_id, .. } => Some(trace_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn trigger_result_audit_records_failure_reason_for_resume_archaeology() {
    // CLI/TUI review on PR #64: jsonl-only readers (e.g. `theway --resume`, /diag, log
    // tooling) must see WHY a sub-agent failed without replaying the in-memory event bus.
    // Verify the audit Custom entry carries `reason`.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Slow stream so we have a window to abort.
    let stream_fn_val: StreamFn = Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
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
    });
    stream_fn = Some(stream_fn_val);
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
        .handle_trigger(sample_trigger("k-reason", "trace-reason"))
        .await;
    wait_for_event(&events, 2, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerExecutionStarted { trace_id, .. }
                if trace_id == "trace-reason" =>
            {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("ExecutionStarted first");
    executor.abort_trigger("trace-reason");
    wait_for_event(&events, 3, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerFailed { trace_id, .. } if trace_id == "trace-reason" => Some(()),
            _ => None,
        })
    })
    .await
    .expect("TriggerFailed within 3s");

    let entries = session.entries().await.unwrap();
    let data = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data.clone(),
            _ => None,
        })
        .expect("trigger_result data");
    assert_eq!(data["success"].as_bool(), Some(false));
    assert_eq!(
        data["reason"].as_str(),
        Some("aborted"),
        "trigger_result must persist failure reason so jsonl-only readers see WHY: {data:?}"
    );
    assert!(
        data["cost_usd"].is_null(),
        "5a does not measure cost — null is honest; 0.0 was misleading"
    );
}

#[tokio::test]
async fn trigger_result_summary_truncation_handles_multibyte_codepoint_via_production_path() {
    // Per @CLI-TUI-Dev-Lead's second PR #64 review: the previous test re-implemented the
    // boundary-walk logic locally, which proves the algorithm is sound but doesn't exercise
    // the production `last_assistant_text` helper. Drive the real `handle_trigger` →
    // sub-agent spawn → `last_assistant_text` → `trigger_result` audit path with a CJK-only
    // body engineered to land the 4 KiB cap mid-codepoint. The pre-fix code would panic
    // (turning the spawn task into a silent abort with no audit); the fixed code produces
    // a truncated summary ending in the truncation marker.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    // 你 is 3 bytes in UTF-8. 1366 copies = 4098 bytes — the 4096-byte cap lands inside the
    // 1366th codepoint. We `Box::leak` because faux_stream_fn takes a `&'static str`.
    let stream_fn = {
        let huge_text: &'static str = Box::leak(("你".repeat(1366)).into_boxed_str());
        Some(faux_stream_fn(huge_text))
    };
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
        .handle_trigger(sample_trigger("k-utf8-trunc", "trace-utf8-trunc"))
        .await;
    // The production path must complete (not panic). If `last_assistant_text` panicked
    // mid-spawn the TriggerCompleted event would never fire.
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted {
                trace_id, summary, ..
            } if trace_id == "trace-utf8-trunc" => Some(summary.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerCompleted must fire — pre-fix code would panic and abort the task");

    // trigger_result audit's summary must be valid UTF-8 (otherwise serde_json would have
    // refused to encode it on the way to JSONL) AND must end with the truncation marker.
    let entries = session.entries().await.unwrap();
    let data = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data.clone(),
            _ => None,
        })
        .expect("trigger_result audit");
    let summary = data["summary"]
        .as_str()
        .expect("summary must be a string (proves valid UTF-8 round-trip through serde_json)");
    assert!(
        summary.ends_with("…[truncated]"),
        "summary must be capped with truncation marker; got len={} ending={:?}",
        summary.len(),
        summary.chars().rev().take(15).collect::<String>(),
    );
    // Per @QA-Release-Lead's PR #65 review: the **final** body (including marker) must
    // respect the 4 KiB cap. Previously we only constrained the pre-marker portion which
    // let the total grow beyond the documented boundary by the marker length.
    assert!(
        summary.len() <= 4096,
        "final summary (including truncation marker) must respect 4 KiB cap; got {}",
        summary.len()
    );
    let body_only = summary.trim_end_matches("…[truncated]");
    // And the truncated text is still composed of valid `你` codepoints (no half codepoint
    // bytes survived).
    assert!(
        body_only.chars().all(|c| c == '你'),
        "truncation MUST land on a char boundary; got non-你 chars in body"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Promotion — RFC 1 sub-PR 5b (PromoteAction::PromoteSummaryNow + template engine +
// trigger_promotion audit + promote_requires_approval fail-closed)
// ─────────────────────────────────────────────────────────────────────────────────────────

fn promoting_action_hook(
    template: Option<String>,
    require_approval: bool,
) -> theway::trigger_engine::execution::BeforeTriggerActionHook {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
    };
    let hook: BeforeTriggerActionHook =
        Arc::new(move |ctx: BeforeTriggerActionContext, _cancel| {
            let template = template.clone();
            Box::pin(async move {
                TriggerAction {
                    prompt: format!(
                        "{} fired: {}",
                        ctx.trigger.source_label, ctx.trigger.event_label
                    ),
                    promote: PromoteAction::PromoteSummaryNow {
                        template_body: template,
                    },
                    promote_requires_approval: require_approval,
                    delivery: theway::trigger_engine::execution::TriggerDelivery::SubAgent,
                }
            })
        });
    hook
}

/// Acceptance #8 from the #20 amendment: no `PromoteAction` configured → parent transcript
/// stays stable. Only `trigger` + `trigger_result` Custom entries appear; no message-typed
/// entries beyond the user prompt + sub-agent transcript (which goes to sub-session, not
/// parent).
#[tokio::test]
async fn no_promote_action_leaves_parent_transcript_stable() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("ok"));
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
        .handle_trigger(sample_trigger("k-no-promote", "trace-no-promote"))
        .await;
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-no-promote" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("must complete");

    let entries = session.entries().await.unwrap();
    // Must NOT contain any Message entries (sub-agent transcript lives in sub-session,
    // promotion didn't fire so nothing inserted).
    let has_message = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(
        !has_message,
        "no promote → parent transcript MUST be empty of Message entries; got {} entries",
        entries.len()
    );
    // Must NOT have a trigger_promotion audit either.
    let has_promotion_audit = entries.iter().any(|e| {
        matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        )
    });
    assert!(
        !has_promotion_audit,
        "no promote → no trigger_promotion audit"
    );

    let evs = events.lock().unwrap().clone();
    let promoted = evs
        .iter()
        .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. }));
    let pending = evs
        .iter()
        .any(|e| matches!(e, TriggerEvent::PromotionPending { .. }));
    assert!(!promoted && !pending, "no promote → no promotion events");
}

/// Inject-summary delivery hook: returns [`TriggerDelivery::InjectSummary`] with a verbatim
/// `{{trigger.payload_summary}}` template (the source-as-feed shape used by MCP servers
/// configured with `inject_summary = true`). Falls back to `PromoteAction::None` when the
/// push carried no summary.
fn inject_summary_action_hook() -> theway::trigger_engine::execution::BeforeTriggerActionHook {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
        TriggerDelivery,
    };
    let hook: BeforeTriggerActionHook =
        Arc::new(move |ctx: BeforeTriggerActionContext, _cancel| {
            let has_summary = ctx.trigger.payload_summary.is_some();
            Box::pin(async move {
                TriggerAction {
                    prompt: String::new(),
                    promote: if has_summary {
                        PromoteAction::PromoteSummaryNow {
                            template_body: Some("{{trigger.payload_summary}}".into()),
                        }
                    } else {
                        PromoteAction::None
                    },
                    promote_requires_approval: false,
                    delivery: TriggerDelivery::InjectSummary,
                }
            })
        });
    hook
}

/// Inject-summary delivery skips the sub-agent entirely and injects `payload_summary`
/// verbatim (with the engine-enforced `[Trigger ]` prefix). The `trigger_result` audit
/// records `message_count: 0` + `delivery: "inject_summary"` + `cost_usd: 0.0` — proof no
/// sub-agent ran. Even with a `stream_fn` configured, its output must never appear.
#[tokio::test]
async fn inject_summary_skips_subagent_and_injects_payload_summary() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("SUBAGENT RAN — must not appear"));
    before_trigger_action = Some(inject_summary_action_hook());
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
        .handle_trigger(sample_trigger("k-inject", "trace-inject"))
        .await;

    let inserted_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-inject" => Some(inserted_entry_id.clone()),
            _ => None,
        })
    })
    .await
    .expect("inject must promote");

    let entries = session.entries().await.unwrap();

    // Injected Message::User body = payload summary verbatim, with the engine prefix.
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_id => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inject must insert a parent user message");
    assert!(
        body.starts_with("[Trigger "),
        "must carry engine [Trigger] prefix: {body}"
    );
    assert!(
        body.contains("PR #42 merged"),
        "must inject verbatim payload summary: {body}"
    );
    assert!(
        !body.contains("SUBAGENT RAN"),
        "sub-agent output must not appear in an inject delivery: {body}"
    );

    // trigger_result audit proves the sub-agent path was skipped.
    let result_audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_result" => Some(d.clone()),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    assert_eq!(result_audit["message_count"], serde_json::json!(0));
    assert_eq!(
        result_audit["delivery"],
        serde_json::json!("inject_summary")
    );
    assert_eq!(result_audit["cost_usd"], serde_json::json!(0.0));
}

/// Inject-summary with a `None` payload summary promotes nothing (no `Message` inserted, no
/// `trigger_promotion` audit) but still completes via the inject path (no sub-agent).
#[tokio::test]
async fn inject_summary_without_payload_summary_promotes_nothing() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("unused"));
    before_trigger_action = Some(inject_summary_action_hook());
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

    let mut trigger = sample_trigger("k-inject-none", "trace-inject-none");
    trigger.payload_summary = None;
    let _ = executor.handle_trigger(trigger).await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-inject-none" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("inject path must still complete");

    let entries = session.entries().await.unwrap();
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "no summary → nothing injected"
    );
    assert!(
        !entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        )),
        "no summary → no trigger_promotion audit"
    );
    let evs = events.lock().unwrap().clone();
    assert!(
        !evs.iter()
            .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. })),
        "no summary → no TriggerPromoted"
    );
}

/// Inject-and-run delivery hook: injects `prompt` into the parent conversation and runs one
/// turn in the parent's context (`TriggerDelivery::InjectAndRun`).
fn inject_and_run_action_hook(
    prompt: &'static str,
) -> theway::trigger_engine::execution::BeforeTriggerActionHook {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
        TriggerDelivery,
    };
    let hook: BeforeTriggerActionHook =
        Arc::new(move |_ctx: BeforeTriggerActionContext, _cancel| {
            Box::pin(async move {
                TriggerAction {
                    prompt: prompt.to_string(),
                    promote: PromoteAction::None,
                    promote_requires_approval: false,
                    delivery: TriggerDelivery::InjectAndRun,
                }
            })
        });
    hook
}

/// Inject-and-run against an IDLE parent: the prompt is appended to the parent conversation
/// (with the `[Trigger ]` prefix) and `TriggerRequestsMainRun` is emitted — but the kernel
/// must NOT run the model itself (the embedder owns the single-tenant parent agent).
#[tokio::test]
async fn inject_and_run_idle_appends_prompt_and_requests_main_run() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("MODEL RAN — must not appear"));
    before_trigger_action = Some(inject_and_run_action_hook("check if I need an umbrella"));
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
        .handle_trigger(sample_trigger("k-iar", "trace-iar"))
        .await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerRequestsMainRun { trace_id } if trace_id == "trace-iar" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("idle inject_and_run must emit TriggerRequestsMainRun");

    let entries = session.entries().await.unwrap();

    // Injected user message = prompt verbatim with the engine `[Trigger ]` prefix.
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } => match &u.content {
                theway_llm_provider::UserContent::Text(s)
                    if s.contains("check if I need an umbrella") =>
                {
                    Some(s.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("inject_and_run must append a parent user message");
    assert!(
        body.starts_with("[Trigger "),
        "must carry engine [Trigger] prefix: {body}"
    );

    // The kernel must NOT run the model: no assistant message, parent stays idle.
    assert!(
        !entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
                ..
            }
        )),
        "kernel must not run the model on the idle parent — that is the embedder's job"
    );
    assert!(
        !harness.agent().is_streaming(),
        "parent must remain idle; the kernel only requested a run"
    );

    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_result" => Some(d.clone()),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    assert_eq!(audit["delivery"], serde_json::json!("inject_and_run"));
    assert_eq!(audit["message_count"], serde_json::json!(0));
    assert_eq!(audit["run_dispatch"], serde_json::json!("main_run_request"));
}

/// Inject-and-run against a STREAMING parent: the prompt is enqueued as a follow-up (the
/// in-flight loop will run it) and `TriggerRequestsMainRun` is NOT emitted — there is already
/// a loop to pick the message up, so the embedder need not schedule anything.
#[tokio::test]
async fn inject_and_run_while_streaming_enqueues_follow_up_no_main_run_event() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    let release = Arc::new(Notify::new());
    let stream_fn_val: StreamFn = {
        let release = release.clone();
        let counter = Arc::new(AtomicUsize::new(0));
        Arc::new(move |_, _, _| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let release = release.clone();
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                if n == 0 {
                    release.notified().await;
                }
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text("resp")],
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
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(stream_fn_val);
    before_trigger_action = Some(inject_and_run_action_hook("react to the event"));
    let mut harness_opts = AgentHarnessOptions::new(faux_model(), session.clone());
    harness_opts.stream_fn = stream_fn.clone();
    let harness = Arc::new(AgentHarness::new(harness_opts));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        None,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let harness_clone = harness.clone();
    let parent_task = tokio::spawn(async move { harness_clone.prompt("kick off parent").await });
    for _ in 0..200 {
        if harness.agent().is_streaming() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        harness.agent().is_streaming(),
        "parent must be streaming before we fire the trigger"
    );

    let _ = executor
        .handle_trigger(sample_trigger("k-iar-s", "trace-iar-s"))
        .await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-iar-s" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("inject_and_run must complete");

    // Streaming → routed through the follow-up queue, NOT a main-run request.
    let evs = events.lock().unwrap().clone();
    assert!(
        !evs.iter().any(|e| matches!(
            e,
            TriggerEvent::TriggerRequestsMainRun { trace_id } if trace_id == "trace-iar-s"
        )),
        "streaming parent already has a loop — must not emit TriggerRequestsMainRun"
    );

    let audit = session
        .entries()
        .await
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_result" => Some(d),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    assert_eq!(audit["delivery"], serde_json::json!("inject_and_run"));
    assert_eq!(audit["run_dispatch"], serde_json::json!("follow_up"));

    // Release the parent stream so the spawned task completes cleanly.
    release.notify_one();
    let _ = parent_task.await;
}

/// Acceptance #9: `PromoteSummaryNow` (no template → built-in default) inserts an audited
/// `Message::User` into the parent jsonl; `trigger_promotion.inserted_entry_id` matches.
#[tokio::test]
async fn promote_summary_now_inserts_audited_parent_entry() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("sub agent reports OK"));
    before_trigger_action = Some(promoting_action_hook(None, false));
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
        .handle_trigger(sample_trigger("k-promote-ok", "trace-promote-ok"))
        .await;

    let promoted_event = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                redaction_status,
                template_name,
                ..
            } if trace_id == "trace-promote-ok" => Some((
                inserted_entry_id.clone(),
                redaction_status.clone(),
                template_name.clone(),
            )),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");
    let (inserted_entry_id, redaction_status, template_name) = promoted_event;
    assert_eq!(redaction_status, "clean");
    // Default built-in template gets stable identifier "default" (per @Tools-MCP-Lead's
    // PR #65 review — the audit contract requires a stable name, not None for default).
    assert_eq!(
        template_name.as_deref(),
        Some("default"),
        "default built-in template must record stable identifier \"default\""
    );

    let entries = session.entries().await.unwrap();

    // The inserted Message::User must exist with the expected id + body containing the
    // default template's text shape.
    let msg = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => Some(u.clone()),
            _ => None,
        })
        .expect("inserted user message must exist in parent jsonl");
    let body = match &msg.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.contains("[Trigger trace-promote-ok]"),
        "default template body must include trace_id prefix; got {body:?}"
    );
    assert!(
        body.contains("sub agent reports OK"),
        "body must include result.summary; got {body:?}"
    );

    // trigger_promotion audit must reference the same inserted_entry_id.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit must exist");
    assert_eq!(audit["state"].as_str(), Some("success"));
    assert_eq!(audit["trace_id"].as_str(), Some("trace-promote-ok"));
    assert_eq!(
        audit["inserted_entry_id"].as_str(),
        Some(inserted_entry_id.as_str())
    );
    assert_eq!(audit["redaction_status"].as_str(), Some("clean"));
}

/// Acceptance #10: template references unknown variable → no insertion, audit `state:
/// "failed"` with `redaction_status: "render_error"`, parent transcript unchanged.
#[tokio::test]
async fn promote_template_unknown_var_fails_closed() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("sub ok"));
    before_trigger_action = Some(promoting_action_hook(
        Some("Hello {{nonexistent_field}}".into()),
        false,
    ));
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
        .handle_trigger(sample_trigger("k-unknown", "trace-unknown"))
        .await;
    // Wait for the promotion-failure PersistenceError reflux.
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::PersistenceError {
                context, message, ..
            } if context == "trigger_promotion" && message.contains("nonexistent_field") => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("PersistenceError with unknown_field reason");

    let entries = session.entries().await.unwrap();
    // No Message::User entries in parent transcript.
    let has_msg = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(!has_msg, "render error → parent transcript unchanged");
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("failed promotion audit");
    assert_eq!(audit["state"].as_str(), Some("failed"));
    assert_eq!(audit["redaction_status"].as_str(), Some("render_error"));
    assert!(audit["inserted_entry_id"].is_null());
}

/// Acceptance #11: template references explicitly forbidden field (e.g.
/// `trigger.payload`) → no insertion, audit `redaction_status: "forbidden_field"`.
#[tokio::test]
async fn promote_template_forbidden_field_fails_closed() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    before_trigger_action = Some(promoting_action_hook(
        Some("Leaking {{trigger.payload}}".into()),
        false,
    ));
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
        .handle_trigger(sample_trigger("k-forbid", "trace-forbid"))
        .await;
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::PersistenceError {
                context, message, ..
            } if context == "trigger_promotion" && message.contains("trigger.payload") => Some(()),
            _ => None,
        })
    })
    .await
    .expect("PersistenceError with forbidden_field reason");

    let entries = session.entries().await.unwrap();
    let has_msg = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(!has_msg);
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("failed promotion audit");
    assert_eq!(audit["state"].as_str(), Some("failed"));
    assert_eq!(audit["redaction_status"].as_str(), Some("forbidden_field"));
}

/// Acceptance #13: `promote_requires_approval = true` + no CLI approval command shipped =
/// fail-closed to pending. `trigger_promotion.state: "pending"`, `PromotionPending` event,
/// parent transcript unchanged.
#[tokio::test]
async fn promote_requires_approval_fails_closed_to_pending() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    before_trigger_action = Some(promoting_action_hook(None, true));
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
        .handle_trigger(sample_trigger("k-pending", "trace-pending"))
        .await;
    let pending = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::PromotionPending {
                trace_id, preview, ..
            } if trace_id == "trace-pending" => Some(preview.clone()),
            _ => None,
        })
    })
    .await
    .expect("PromotionPending must fire");
    let preview = pending.expect("preview body should be Some when render succeeded");
    assert!(preview.contains("[Trigger trace-pending]"));

    let entries = session.entries().await.unwrap();
    let has_msg = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(
        !has_msg,
        "promote_requires_approval=true must NOT insert into parent transcript without explicit approval"
    );
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("pending promotion audit");
    assert_eq!(audit["state"].as_str(), Some("pending"));
    assert!(audit["inserted_entry_id"].is_null());

    // Also assert no TriggerPromoted event.
    let evs = events.lock().unwrap().clone();
    let promoted = evs.iter().any(|e| {
        matches!(
            e,
            TriggerEvent::TriggerPromoted { trace_id, .. } if trace_id == "trace-pending"
        )
    });
    assert!(!promoted);
}

/// Acceptance #12: summary cap truncation. Large `result.summary` (> 4 KiB) is truncated
/// and `redaction_status: "truncated"` is reflected in both the audit and the event.
///
/// Drive by giving the faux stream a huge assistant body so `last_assistant_text` already
/// truncates the summary down to 4 KiB. Then the rendered template body (containing that
/// summary) will exceed `PROMOTION_BODY_CAP_BYTES` and trigger the body-cap truncation.
#[tokio::test]
async fn promote_summary_truncation_records_redaction_status() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    // ~6 KiB assistant text.
    let stream_fn = {
        let huge_text: &'static str = Box::leak(("X".repeat(6 * 1024)).into_boxed_str());
        Some(faux_stream_fn(huge_text))
    };
    before_trigger_action = Some(promoting_action_hook(None, false));
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
        .handle_trigger(sample_trigger("k-trunc", "trace-trunc"))
        .await;
    let evt = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                redaction_status,
                inserted_entry_id,
                ..
            } if trace_id == "trace-trunc" => {
                Some((redaction_status.clone(), inserted_entry_id.clone()))
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted (truncated) must fire");
    let (redaction, inserted_id) = evt;
    assert_eq!(redaction, "truncated");

    // The inserted message must be capped (≤ 4 KiB + the marker bytes).
    let entries = session.entries().await.unwrap();
    let msg = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_id => Some(u.clone()),
            _ => None,
        })
        .expect("inserted user message");
    let body = match &msg.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.ends_with("…[truncated]"),
        "truncated body must end with truncation marker"
    );
    // Audit must match.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("audit");
    assert_eq!(audit["redaction_status"].as_str(), Some("truncated"));
}

/// Provider-Auth review on PR #65: inline `PromoteSummaryNow { template_body }` MUST NOT
/// be stored as `template_name` in the `trigger_promotion` audit / events. Audit identity
/// shape: `"default"` for built-in, `"inline:{hash[..8]}"` for hook-supplied bodies, with
/// the full SHA-256 in `template_hash` for verification.
#[tokio::test]
async fn promote_inline_template_body_is_not_persisted_as_template_name() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("subagent text"));
    let inline_body = "Custom RFC4-style prompt: {{trigger.source_label}} → {{result.summary}}";
    before_trigger_action = Some(promoting_action_hook(Some(inline_body.into()), false));
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
        .handle_trigger(sample_trigger("k-inline-name", "trace-inline-name"))
        .await;
    let promoted_template_name = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                template_name,
                ..
            } if trace_id == "trace-inline-name" => Some(template_name.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    let name = promoted_template_name.expect("template_name must be Some");
    assert!(
        name.starts_with("inline:"),
        "inline template MUST be identified via inline:hash prefix, got {name:?}"
    );
    assert_eq!(
        name.len(),
        "inline:".len() + 8,
        "inline name is `inline:` + first 8 chars of sha256(body); got {name:?}"
    );
    assert!(
        !name.contains(inline_body),
        "template_name MUST NOT contain raw body: got {name:?}"
    );

    // Audit shows the same shape + a full template_hash for cross-process verification.
    let entries = session.entries().await.unwrap();
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit");
    assert_eq!(audit["template_name"].as_str(), Some(name.as_str()));
    let template_hash = audit["template_hash"]
        .as_str()
        .expect("template_hash must be Some(hex string)");
    assert_eq!(
        template_hash.len(),
        64,
        "SHA-256 hex must be 64 chars; got {} chars",
        template_hash.len()
    );
    assert!(
        !audit.to_string().contains(inline_body),
        "raw template body MUST NOT appear anywhere in the audit blob: {audit:?}"
    );
}

/// @Tools-MCP-Lead PR #65 review: enforce `[Trigger {trace_id}] ` prefix in the engine,
/// not in template-author discipline. A custom template without the prefix MUST still
/// produce a parent-session entry that starts with the prefix + audit
/// `prefix_injected: true`.
#[tokio::test]
async fn promote_summary_now_custom_template_without_prefix_still_gets_injected() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("subagent text"));
    // Custom template WITHOUT the `[Trigger ...]` prefix — engine must inject one.
    before_trigger_action = Some(promoting_action_hook(
        Some("Bare update from {{trigger.source_label}}: {{result.summary}}".into()),
        false,
    ));
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
        .handle_trigger(sample_trigger("k-prefix-inj", "trace-prefix-inj"))
        .await;
    let inserted_entry_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-prefix-inj" => Some(inserted_entry_id.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    let entries = session.entries().await.unwrap();
    let msg = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => Some(u.clone()),
            _ => None,
        })
        .expect("inserted user message");
    let body = match &msg.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.starts_with("[Trigger trace-prefix-inj] "),
        "engine MUST inject the trigger prefix on templates that don't include one; got: {body:?}"
    );
    assert!(
        body.contains("Bare update from MCP github"),
        "custom template body MUST still be rendered after the injected prefix; got: {body:?}"
    );

    // Audit reflects prefix_injected = true.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit");
    assert_eq!(audit["prefix_injected"].as_bool(), Some(true));

    // Idempotency check: a template that ALREADY starts with [Trigger should NOT get
    // double-prefixed (covered by the default-template test where audit prefix_injected
    // ought to be false). Verified here implicitly: if the engine doubled the prefix,
    // body would start with `[Trigger trace-prefix-inj] [Trigger ...]`.
    assert!(
        !body.starts_with("[Trigger trace-prefix-inj] [Trigger"),
        "prefix injection MUST be idempotent (no double `[Trigger `); got: {body:?}"
    );
}

#[tokio::test]
async fn promote_default_template_does_not_get_double_prefixed() {
    // Idempotency: the default template already starts with `[Trigger {{trace_id}}]`, so
    // the engine must NOT prepend a second prefix. Audit reflects prefix_injected = false.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    before_trigger_action = Some(promoting_action_hook(None, false));
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
        .handle_trigger(sample_trigger("k-default-pfx", "trace-default-pfx"))
        .await;
    // Wait for completion.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let entries = session.entries().await.unwrap();
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit");
    assert_eq!(audit["prefix_injected"].as_bool(), Some(false));
    let user_msg_body = entries.iter().find_map(|e| match e {
        SessionTreeEntry::Message {
            message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
            ..
        } => match &u.content {
            theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    });
    let body = user_msg_body.expect("inserted user message");
    let prefix_occurrences = body.matches("[Trigger trace-default-pfx]").count();
    assert_eq!(
        prefix_occurrences, 1,
        "default template MUST NOT be double-prefixed; got body={body:?}"
    );
}

/// QA review on PR #65 a98c70b: `ensure_trigger_prefix` did `body.starts_with("[Trigger ")`
/// which would accept ANY `[Trigger ...]` prefix — including one a malicious template
/// embeds with a fake trace id. Fix: require the exact `[Trigger {trace_id}] ` form;
/// otherwise still inject the real prefix so the authoritative trace id wins.
#[tokio::test]
async fn promote_template_with_stale_trigger_prefix_still_gets_real_trace_id_prepended() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    // Template carries a STALE / spoofed `[Trigger evil-trace-id]` prefix. The engine
    // must still prepend `[Trigger trace-real]` so the actual trace id is the first one
    // a reader sees; the stale one becomes embedded text.
    before_trigger_action = Some(promoting_action_hook(
        Some("[Trigger evil-trace-id] spoofed body for {{result.summary}}".into()),
        false,
    ));
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
        .handle_trigger(sample_trigger("k-stale-prefix", "trace-real"))
        .await;
    let inserted_entry_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-real" => Some(inserted_entry_id.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    let entries = session.entries().await.unwrap();
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inserted user message");

    // Must start with the REAL trace id, not the stale one.
    assert!(
        body.starts_with("[Trigger trace-real] "),
        "real trace id MUST be prepended; got body={body:?}"
    );
    // The stale prefix appears as embedded text further in the body — proves the engine
    // didn't trust the user-supplied prefix.
    assert!(
        body.contains("[Trigger evil-trace-id]"),
        "stale prefix should remain as embedded text, body={body:?}"
    );
    // Audit reflects the real injection happened.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("audit");
    assert_eq!(audit["prefix_injected"].as_bool(), Some(true));
}

/// QA review on PR #65 a98c70b: previous truncation appended the marker AFTER cutting to
/// the cap, so the final body length = cap + marker.len() (~12 bytes over). Fix: cap is
/// the FINAL length including the marker.
#[tokio::test]
async fn promote_summary_truncation_final_length_includes_marker_under_cap() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    // Huge assistant text that triggers `last_assistant_text` truncation, which then feeds
    // a huge `{{result.summary}}` into the promotion template body. Both truncation sites
    // must respect the 4 KiB cap including marker.
    let stream_fn = {
        let huge_text: &'static str = Box::leak(("X".repeat(10 * 1024)).into_boxed_str());
        Some(faux_stream_fn(huge_text))
    };
    before_trigger_action = Some(promoting_action_hook(None, false));
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
        .handle_trigger(sample_trigger("k-cap-final", "trace-cap-final"))
        .await;
    let inserted_entry_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                redaction_status,
                ..
            } if trace_id == "trace-cap-final" && redaction_status == "truncated" => {
                Some(inserted_entry_id.clone())
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted (truncated) must fire");

    let entries = session.entries().await.unwrap();
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inserted body");

    assert!(
        body.ends_with("…[truncated]"),
        "final body must end with truncation marker"
    );
    // The fix's contract: the FINAL body (including marker) is ≤ cap.
    assert!(
        body.len() <= 4096,
        "final inserted body (including marker) MUST respect 4 KiB cap; got {} bytes",
        body.len()
    );

    // Same invariant applies to trigger_result.summary that feeds into the template.
    let summary = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data
                .as_ref()
                .and_then(|d| d["summary"].as_str().map(String::from)),
            _ => None,
        })
        .expect("trigger_result.summary");
    assert!(
        summary.len() <= 4096,
        "trigger_result.summary (including marker) MUST respect 4 KiB cap; got {} bytes",
        summary.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// PromotionCondition — structured authorization gate for
// PromoteAction::PromoteSummaryWhenResultDetailsMatch. These tests pin the runtime
// contract directly (not through coding-agent's dynamic.rs path). Coverage:
//   - pointer-missing / value-not-array / empty-intersection → distinct skip reasons
//   - matching path → returns the intersection
//   - skip reasons stringify to stable audit identifiers
// ─────────────────────────────────────────────────────────────────────────────────────────

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

/// Promotion fired while the parent agent is mid-stream MUST NOT double-persist or land
/// out of order. Pins QA's PR #67 blocker: the streaming branch hands off to the loop's
/// follow-up queue (single persistence path via the session listener); audit reflects
/// `state: "queued"` and `inserted_entry_id: null` because the entry ID is only known
/// after the loop drains. Once the parent stream releases, the session must contain
/// exactly one promoted Message::User AND it must come AFTER the parent's assistant
/// response — never before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promote_while_parent_is_streaming_routes_through_follow_up_single_write() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    // Controllable stream factory. The first call (parent's initial prompt) waits on
    // `release` so we can race a trigger promotion against the in-flight stream. All
    // later calls (sub-agent inside `handle_trigger`, parent's follow-up turn) resolve
    // immediately so the test doesn't deadlock waiting on them.
    fn controllable_stream_fn(release: Arc<Notify>) -> StreamFn {
        let counter = Arc::new(AtomicUsize::new(0));
        Arc::new(move |_, _, _| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let release = release.clone();
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                if n == 0 {
                    release.notified().await;
                }
                let body = match n {
                    0 => "parent response",
                    _ => "auxiliary response",
                };
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text(body)],
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

    let release = Arc::new(Notify::new());
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(controllable_stream_fn(release.clone()));
    let parent_stream_fn = stream_fn.clone();
    before_trigger_action = Some({
        let hook: theway::trigger_engine::execution::BeforeTriggerActionHook = Arc::new(
            move |ctx: theway::trigger_engine::execution::BeforeTriggerActionContext, _cancel| {
                Box::pin(async move {
                    theway::trigger_engine::execution::TriggerAction {
                        prompt: format!(
                            "{} fired: {}",
                            ctx.trigger.source_label, ctx.trigger.event_label
                        ),
                        // `PromoteSummaryNow` always fires (no conditional gate); we're
                        // testing the persistence/ordering branch in `apply_promotion`,
                        // not the condition evaluator.
                        promote:
                            theway::trigger_engine::execution::PromoteAction::PromoteSummaryNow {
                                template_body: None,
                            },
                        promote_requires_approval: false,
                        delivery: theway::trigger_engine::execution::TriggerDelivery::SubAgent,
                    }
                })
            },
        );
        hook
    });
    let mut harness_opts = AgentHarnessOptions::new(faux_model(), session.clone());
    harness_opts.stream_fn = parent_stream_fn;
    let harness = Arc::new(AgentHarness::new(harness_opts));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        None,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    // Spawn parent prompt in background; it'll block at the stream's first `notified().await`
    // until we release. `is_streaming()` should be true during this window.
    let harness_clone = harness.clone();
    let parent_task = tokio::spawn(async move { harness_clone.prompt("kick off parent").await });

    // Wait for the parent to actually enter the streaming state.
    for _ in 0..200 {
        if harness.agent().is_streaming() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        harness.agent().is_streaming(),
        "parent agent must be streaming before we fire the trigger",
    );

    // Fire the trigger while parent is still mid-stream. The sub-agent is built with the
    // same stream_fn but its call is `n=1` so resolves immediately.
    let _ = executor
        .handle_trigger(sample_trigger("k-streaming", "trace-streaming"))
        .await;

    // Wait for `TriggerPromoted` so we know `apply_promotion` ran while parent was still
    // streaming. (Doesn't release the parent stream yet.)
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted { trace_id, .. } if trace_id == "trace-streaming" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    // The promotion ran during streaming → audit MUST be the queued shape, not success.
    // No Message::User in session yet — the loop hasn't drained the follow-up.
    let mid_entries = session.entries().await.unwrap();
    let mid_promotion_audit = mid_entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit must exist during streaming case");
    assert_eq!(
        mid_promotion_audit["state"], "queued",
        "streaming-branch promotion audit must report state=queued, got {mid_promotion_audit}",
    );
    assert!(
        mid_promotion_audit["inserted_entry_id"].is_null(),
        "inserted_entry_id MUST be null while message is queued (ID only known after loop drains)",
    );
    let mid_user_count = mid_entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                SessionTreeEntry::Message {
                    message: AgentMessage::Llm(theway_llm_provider::Message::User(_)),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        mid_user_count, 1,
        "before parent stream releases, session should have exactly 1 user message (the parent's initial prompt); got {mid_user_count}",
    );

    // Release the parent's first stream → loop appends assistant response → drains
    // follow_up → emits the promoted user message → session listener writes once.
    // Subsequent stream calls (parent's continuation after follow_up drain) resolve
    // immediately via `n != 0` branch.
    release.notify_one();
    let _ = parent_task.await.expect("parent task should join");
    // Allow listener writes to flush (subscribe_harness uses spawned tasks).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let final_entries = session.entries().await.unwrap();

    // Single persistence path: exactly TWO user messages now (initial prompt + promoted),
    // never three or more.
    let user_msgs: Vec<&SessionTreeEntry> = final_entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                SessionTreeEntry::Message {
                    message: AgentMessage::Llm(theway_llm_provider::Message::User(_)),
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        user_msgs.len(),
        2,
        "single persistence path: expected exactly 2 user messages (initial prompt + promoted), got {}",
        user_msgs.len(),
    );

    // Deterministic order: the promoted user message MUST come AFTER the parent's
    // assistant response in the session JSONL.
    let positions: Vec<(usize, &str)> = final_entries
        .iter()
        .enumerate()
        .filter_map(|(idx, e)| match e {
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } => match &u.content {
                theway_llm_provider::UserContent::Text(t) if t.starts_with("[Trigger ") => {
                    Some((idx, "promoted"))
                }
                _ => None,
            },
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
                ..
            } => Some((idx, "assistant")),
            _ => None,
        })
        .collect();
    let assistant_idx = positions
        .iter()
        .find(|(_, k)| *k == "assistant")
        .map(|(i, _)| *i);
    let promoted_idx = positions
        .iter()
        .find(|(_, k)| *k == "promoted")
        .map(|(i, _)| *i);
    assert!(
        assistant_idx.is_some() && promoted_idx.is_some(),
        "both assistant response and promoted user message must be persisted: {positions:?}",
    );
    assert!(
        promoted_idx.unwrap() > assistant_idx.unwrap(),
        "promoted user message MUST come AFTER the in-flight assistant response in session JSONL; got positions {positions:?}",
    );
}
