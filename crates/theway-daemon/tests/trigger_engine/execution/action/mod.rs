//! Tests for `trigger_engine::execution::action` — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::trigger_engine::execution::types::PromoteAction;
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, TriggerAuthority,
    TriggerSource,
};
use theway_core::{AgentOptions, MemorySessionStorage, SessionError, SessionErrorCode, SessionStorage, SessionTreeEntry};

fn trigger_with_source(source: TriggerSource) -> Trigger {
    Trigger {
        source,
        source_kind: SourceKind::Local,
        source_label: "local:test".into(),
        event_label: "test event".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: Some("payload summary".into()),
        payload: None,
        idempotency_key: "idem-1".into(),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: "trace-1".into(),
        authority: TriggerAuthority {
            principal_id: "principal-1".into(),
            principal_label: "Principal One".into(),
            credential_scope: CredentialScope::User,
            allowed_source_actions: vec!["read".into()],
            expires_at: None,
        },
        received_at: chrono::Utc::now(),
    }
}

fn mcp_trigger() -> Trigger {
    let mut t = trigger_with_source(TriggerSource::Mcp {
        server_name: "github".into(),
        method: "notifications/pr.merged".into(),
    });
    t.source_kind = SourceKind::Mcp;
    t.source_label = "MCP github".into();
    t
}

fn runtime_snapshot() -> TriggerRuntimeSnapshot {
    TriggerRuntimeSnapshot {
        dedup_entries: 0,
        active_traces: 0,
        accepted_total: 0,
        deduped_total: 0,
        cycle_suppressed_total: 0,
    }
}

fn listener_vec(
    events: &Arc<std::sync::Mutex<Vec<TriggerEvent>>>,
) -> Arc<Mutex<Vec<TriggerListener>>> {
    let sink = events.clone();
    Arc::new(Mutex::new(vec![Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    })]))
}

fn action_hook(action: TriggerAction) -> BeforeTriggerActionHook {
    Arc::new(move |_ctx, _cancel| {
        let action = action.clone();
        Box::pin(async move { action })
    })
}

fn inject_summary_hook(promote: PromoteAction) -> BeforeTriggerActionHook {
    action_hook(TriggerAction {
        prompt: String::new(),
        promote,
        promote_requires_approval: false,
        delivery: TriggerDelivery::InjectSummary,
    })
}

fn inject_and_run_hook(prompt: &'static str) -> BeforeTriggerActionHook {
    action_hook(TriggerAction {
        prompt: prompt.into(),
        promote: PromoteAction::None,
        promote_requires_approval: false,
        delivery: TriggerDelivery::InjectAndRun,
    })
}

fn running_registry() -> Arc<Mutex<std::collections::HashMap<String, RunningTriggerHandle>>> {
    Arc::new(Mutex::new(std::collections::HashMap::new()))
}

fn session_with_memory() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn parent_agent() -> Arc<Agent> {
    Arc::new(Agent::new(AgentOptions::default()))
}

struct FailingAppendStorage {
    inner: Arc<MemorySessionStorage>,
}

#[async_trait::async_trait]
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

fn failing_session() -> Session {
    Session::new(
        Arc::new(FailingAppendStorage {
            inner: Arc::new(MemorySessionStorage::new()),
        }) as Arc<dyn SessionStorage>,
    )
}

#[tokio::test]
async fn inject_summary_writes_result_audit_and_promotes() {
    let session = session_with_memory();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let hook = inject_summary_hook(PromoteAction::PromoteSummaryNow {
        template_body: Some("{{trigger.payload_summary}}".into()),
    });

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session.clone(),
        agent,
        running_registry(),
        Some(hook),
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-1"
    )));
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::TriggerCompleted { trace_id, cost_usd, .. }
            if trace_id == "trace-1" && *cost_usd == Some(0.0)
    )));
    let promoted = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-1" => Some(inserted_entry_id.clone()),
            _ => None,
        })
        .expect("inject_summary must promote");

    let entries = session.entries().await.unwrap();
    let result = entries
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
    assert_eq!(result["delivery"], "inject_summary");
    assert_eq!(result["message_count"], 0);
    assert_eq!(result["cost_usd"], 0.0);

    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &promoted => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inject_summary must insert a parent message");
    assert!(body.starts_with("[Trigger trace-1] "), "{body}");
    assert!(body.contains("payload summary"), "{body}");
}

#[tokio::test]
async fn inject_summary_without_summary_and_no_promote_completes_without_promotion() {
    let session = session_with_memory();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let mut trigger = mcp_trigger();
    trigger.payload_summary = None;
    let hook = inject_summary_hook(PromoteAction::None);

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session.clone(),
        agent,
        running_registry(),
        Some(hook),
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::TriggerCompleted { trace_id, summary, .. }
            if trace_id == "trace-1" && summary.is_none()
    )));
    assert!(
        !evs.iter()
            .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. })),
        "no summary and PromoteAction::None -> no promotion event"
    );

    let entries = session.entries().await.unwrap();
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "no summary -> no message injected"
    );
    assert!(
        !entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        )),
        "no summary -> no trigger_promotion audit"
    );
}

#[tokio::test]
async fn inject_and_run_idle_appends_prompt_and_requests_main_run() {
    let session = session_with_memory();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let hook = inject_and_run_hook("check if I need an umbrella");

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session.clone(),
        agent.clone(),
        running_registry(),
        Some(hook),
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::TriggerRequestsMainRun { trace_id } if trace_id == "trace-1"
    )));

    let entries = session.entries().await.unwrap();
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
        .expect("inject_and_run must append a parent message");
    assert!(body.starts_with("[Trigger trace-1] "), "{body}");

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
    assert_eq!(audit["delivery"], "inject_and_run");
    assert_eq!(audit["run_dispatch"], "main_run_request");
    assert_eq!(audit["message_count"], 0);

    assert!(
        !agent.is_streaming(),
        "idle parent must not be marked streaming by injection"
    );
}

#[tokio::test]
async fn inject_and_run_streaming_enqueues_follow_up_without_main_run_event() {
    let session = session_with_memory();
    let agent = parent_agent();
    agent.state().is_streaming = true;
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let hook = inject_and_run_hook("react to the event");

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session.clone(),
        agent.clone(),
        running_registry(),
        Some(hook),
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(
        !evs.iter().any(|e| matches!(
            e,
            TriggerEvent::TriggerRequestsMainRun { trace_id } if trace_id == "trace-1"
        )),
        "streaming parent must not request a main run"
    );
    let entries = session.entries().await.unwrap();
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "streaming parent must not receive a direct session write"
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
    assert_eq!(audit["run_dispatch"], "follow_up");
}

#[tokio::test]
async fn inject_and_run_idle_append_error_emits_persistence_error() {
    let session = failing_session();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let hook = inject_and_run_hook("do the thing");

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session,
        agent,
        running_registry(),
        Some(hook),
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, .. } if context == "trigger_inject_and_run"
    )));
}

#[tokio::test]
async fn inject_summary_append_error_emits_persistence_errors() {
    let session = failing_session();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let hook = inject_summary_hook(PromoteAction::PromoteSummaryNow {
        template_body: Some("{{trigger.payload_summary}}".into()),
    });

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session,
        agent,
        running_registry(),
        Some(hook),
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, .. } if context == "trigger_result"
    )));
    // The promotion success path tries `append_message` first; with a failing
    // storage backend that failure must be refluxed too.
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, .. }
            if context == "trigger_promotion" && message.contains("append failed")
    )));
}

#[tokio::test]
async fn sub_agent_success_writes_trigger_result_and_clears_running_registry() {
    let session = session_with_memory();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let registry = running_registry();

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session.clone(),
        agent,
        registry.clone(),
        None,
        runtime_snapshot(),
        Some(faux_model()),
        String::new(),
        Vec::new(),
        None,
        Some(faux_stream_fn("sub-agent says hello")),
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-1"
    )));
    let summary = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerCompleted {
                trace_id, summary, ..
            } if trace_id == "trace-1" => Some(summary.clone()),
            _ => None,
        })
        .expect("sub-agent success must emit TriggerCompleted");
    assert_eq!(summary.as_deref(), Some("sub-agent says hello"));

    assert!(
        registry.lock().is_empty(),
        "running registry must be cleared after completion"
    );

    let entries = session.entries().await.unwrap();
    let result = entries
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
    assert_eq!(result["success"], true);
    assert_eq!(result["summary"], "sub-agent says hello");
    assert_eq!(result["message_count"], 2);
}

#[tokio::test]
async fn sub_agent_failure_without_model_emits_failed_and_audits_reason() {
    let session = session_with_memory();
    let agent = parent_agent();
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let registry = running_registry();

    run_trigger_action(
        trigger,
        "trace-1".into(),
        "local:test".into(),
        "test event".into(),
        listeners,
        session.clone(),
        agent,
        registry.clone(),
        None,
        runtime_snapshot(),
        None,
        String::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    let reason = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerFailed { trace_id, reason } if trace_id == "trace-1" => {
                Some(reason.clone())
            }
            _ => None,
        })
        .expect("sub-agent failure must emit TriggerFailed");
    assert!(reason.contains("no model"), "{reason}");

    assert!(
        registry.lock().is_empty(),
        "running registry must be cleared after failure"
    );

    let entries = session.entries().await.unwrap();
    let result = entries
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
    assert_eq!(result["success"], false);
    assert!(result["reason"].as_str().unwrap().contains("no model"));
}

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream_fn(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![theway_llm_provider::ContentBlock::text(text)],
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
