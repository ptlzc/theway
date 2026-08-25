//! Additional focused coverage for `trigger_engine::execution` public paths:
//! unsubscribe handles, prompt audit failure reflux, long-label capping,
//! empty action-class fallback, `abort_all_triggers`, and long banner previews.

use std::sync::Arc;

use async_trait::async_trait;
use theway_core::{
    MemorySessionStorage, Session, SessionError, SessionErrorCode, SessionStorage, SessionTreeEntry,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, StopReason, Usage,
};

use super::*;

#[tokio::test]
async fn unsubscribe_removes_listener_and_stops_future_events() {
    // Arrange
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    // Act: remove the listener before handling any trigger.
    unsub();

    let _ = executor
        .handle_trigger(sample_trigger("k-unsub", "trace-unsub"))
        .await;

    // Assert: the unsubscribed listener must not receive any events.
    assert!(
        events.lock().unwrap().is_empty(),
        "unsubscribed listener must not receive events"
    );
}

#[tokio::test]
async fn prompt_request_caps_long_labels_and_falls_back_for_empty_action_class() {
    use theway_daemon::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook, OnTriggerPromptHook,
        TriggerPromptDecision,
    };

    // Arrange
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let before_trigger: Option<BeforeTriggerHook> =
        Some(Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
            Box::pin(async move {
                BeforeTriggerDecision::Prompt {
                    reason: "needs review".into(),
                }
            })
        }));
    let seen_request = Arc::new(std::sync::Mutex::new(None));
    let seen_sink = seen_request.clone();
    let on_trigger_prompt: Option<OnTriggerPromptHook> = Some(Arc::new(move |request, _cancel| {
        *seen_sink.lock().unwrap() = Some(request);
        Box::pin(async move { TriggerPromptDecision::Allow })
    }));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        TriggerRuntimeConfig::default(),
        before_trigger,
        on_trigger_prompt,
        None,
        None,
        None,
        None,
    ));

    let mut trigger = sample_trigger("k-long-labels", "trace-long-labels");
    trigger.source_label = "source-".repeat(60); // >200 chars
    trigger.event_label = "event-".repeat(60); // >200 chars
    trigger.authority.principal_id = "principal-".repeat(60); // >200 chars
    trigger.authority.principal_label = "Principal ".repeat(60); // >200 chars
    trigger.payload = Some(serde_json::json!({
        "_meta": {
            "action_class": ""
        }
    }));

    // Act
    let _ = executor.handle_trigger(trigger).await;

    // Assert
    let request = seen_request
        .lock()
        .unwrap()
        .clone()
        .expect("on_trigger_prompt must receive request");
    assert_eq!(request.source_label.chars().count(), 200);
    assert!(request.source_label.ends_with('…'));
    let payload_event_label = request.payload["event_label"].as_str().unwrap();
    assert_eq!(payload_event_label.chars().count(), 200);
    assert!(payload_event_label.ends_with('…'));
    assert_eq!(request.sender_agent_id.chars().count(), 200);
    assert!(request.sender_agent_id.ends_with('…'));
    assert_eq!(request.action_class, payload_event_label);
    assert_eq!(
        request.payload["authority"]["principal_label"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        200
    );
    assert_eq!(
        request.payload["authority"]["principal_id"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        60 * "principal-".len()
    );
}

#[tokio::test]
async fn prompt_audit_append_failure_emits_persistence_error() {
    use theway_daemon::trigger_engine::execution::{
        BeforeTriggerContext, BeforeTriggerDecision, BeforeTriggerHook, OnTriggerPromptHook,
        TriggerPromptDecision,
    };

    // Arrange: a storage that fails every append.
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
    let before_trigger: Option<BeforeTriggerHook> =
        Some(Arc::new(|_ctx: BeforeTriggerContext, _cancel| {
            Box::pin(async move {
                BeforeTriggerDecision::Prompt {
                    reason: "needs review".into(),
                }
            })
        }));
    let on_trigger_prompt: Option<OnTriggerPromptHook> = Some(Arc::new(|_request, _cancel| {
        Box::pin(async move { TriggerPromptDecision::Allow })
    }));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        TriggerRuntimeConfig::default(),
        before_trigger,
        on_trigger_prompt,
        None,
        None,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    // Act
    let _ = executor
        .handle_trigger(sample_trigger("k-prompt-fail", "trace-prompt-fail"))
        .await;

    // Assert
    let evs = events.lock().unwrap().clone();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            TriggerEvent::PersistenceError { context, .. } if context == "trigger_prompt"
        )),
        "prompt audit append failure must emit PersistenceError"
    );
}

#[tokio::test]
async fn abort_all_triggers_cancels_every_in_flight_sub_agent() {
    // Arrange
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let stream_fn: Option<StreamFn> = Some(Arc::new(|_, _, _| {
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
    }));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
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
        .handle_trigger(sample_trigger("k-abort-all-1", "trace-abort-all-1"))
        .await;
    let _ = executor
        .handle_trigger(sample_trigger("k-abort-all-2", "trace-abort-all-2"))
        .await;

    // Wait until both sub-agents have started.
    wait_for_event(&events, 3, |evs| {
        let started = evs
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    TriggerEvent::TriggerExecutionStarted { trace_id, .. }
                        if trace_id == "trace-abort-all-1" || trace_id == "trace-abort-all-2"
                )
            })
            .count();
        if started >= 2 { Some(()) } else { None }
    })
    .await
    .expect("both triggers must start execution");

    // Act
    executor.abort_all_triggers();

    // Assert: both produce TriggerFailed with reason "aborted".
    for trace in ["trace-abort-all-1", "trace-abort-all-2"] {
        let reason = wait_for_event(&events, 3, |evs| {
            evs.iter().find_map(|e| match e {
                TriggerEvent::TriggerFailed { trace_id, reason } if trace_id == trace => {
                    Some(reason.clone())
                }
                _ => None,
            })
        })
        .await
        .expect("aborted trigger must emit TriggerFailed");
        assert_eq!(reason, "aborted");
    }
}

#[tokio::test]
async fn running_snapshot_truncates_long_prompt_preview() {
    use theway_daemon::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
    };

    // Arrange
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let before_trigger_action: Option<BeforeTriggerActionHook> =
        Some(Arc::new(|_ctx: BeforeTriggerActionContext, _cancel| {
            Box::pin(async move {
                TriggerAction {
                    prompt: "long-prompt-".repeat(20),
                    promote: PromoteAction::None,
                    promote_requires_approval: false,
                    delivery: theway_daemon::trigger_engine::execution::TriggerDelivery::SubAgent,
                }
            })
        }));
    let stream_fn: Option<StreamFn> = Some(Arc::new(|_, _, _| {
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
    }));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    // Act
    let _ = executor
        .handle_trigger(sample_trigger("k-preview", "trace-preview"))
        .await;

    // Assert
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let rt = loop {
        let snap = executor.notification_status_snapshot();
        if let Some(rt) = snap.running.iter().find(|r| r.trace_id == "trace-preview") {
            break rt.clone();
        }
        if std::time::Instant::now() > deadline {
            panic!("running snapshot did not include long-preview trigger");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert!(
        rt.prompt_preview.chars().count() == 81,
        "long prompt preview should be capped to 80 chars plus ellipsis; got {:?}",
        rt.prompt_preview
    );
    assert!(rt.prompt_preview.ends_with('…'));
}
