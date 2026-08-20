use super::*;
use crate::agent::compaction::algorithm::{
    CompactAlgorithm, CompactAlgorithmRegistry, SummarizeRequest, SummaryOutcome,
};
use crate::agent::compaction::compaction::CompactionSettings;
use crate::agent::runtime_extensions::ExtensionModelContextProjection;
use async_trait::async_trait;
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionModelContextPlacement,
    ExtensionStateMutation,
};
use theway_llm_provider::Usage;

fn compact_settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: 0,
        keep_recent_tokens: 4,
        algorithm: "builtin".into(),
    }
}

async fn populate(harness: &AgentHarness) {
    harness.prompt("first").await.unwrap();
    harness.prompt("second").await.unwrap();
    harness.prompt("third").await.unwrap();
}

#[tokio::test]
async fn compaction_gate_precedes_committed_success_observation() {
    let port = Arc::new(RecordingPort::default());
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    let harness = harness_with_port(
        port.clone(),
        success_stream(Arc::new(AtomicUsize::new(0))),
        session.clone(),
    );
    *harness.compaction_settings.lock() = compact_settings();
    populate(&harness).await;

    assert!(harness.force_compact(None).await.unwrap());

    let events = port
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ExtensionLifecycleEvent::BeforeCompaction
                    | ExtensionLifecycleEvent::CompactionSucceeded
                    | ExtensionLifecycleEvent::CompactionFailed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            ExtensionLifecycleEvent::BeforeCompaction,
            ExtensionLifecycleEvent::CompactionSucceeded,
        ]
    );
    assert!(session
        .entries()
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry, SessionTreeEntry::Compaction { .. })));
}

#[tokio::test]
async fn denied_compaction_gate_leaves_session_without_compaction_entry() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::BeforeCompaction,
        ExtensionHookClass::Gate,
        ExtensionActionBatch {
            abi_major: ExtensionAbiMajor::V2,
            decision: Some(ExtensionGateDecision::Cancel {
                code: "compaction_disabled".into(),
                message: "disabled by extension".into(),
            }),
            actions: Vec::new(),
        },
    );
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    let harness = harness_with_port(
        port.clone(),
        success_stream(Arc::new(AtomicUsize::new(0))),
        session.clone(),
    );
    *harness.compaction_settings.lock() = compact_settings();
    populate(&harness).await;

    let error = harness.force_compact(None).await.unwrap_err();

    assert!(error.to_string().contains("disabled by extension"));
    assert!(!session
        .entries()
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry, SessionTreeEntry::Compaction { .. })));
    let events = port
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ExtensionLifecycleEvent::BeforeCompaction
                    | ExtensionLifecycleEvent::CompactionSucceeded
                    | ExtensionLifecycleEvent::CompactionFailed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(events, vec![ExtensionLifecycleEvent::BeforeCompaction]);
}

#[tokio::test]
async fn compaction_provider_failure_publishes_failure_without_commit() {
    let port = Arc::new(RecordingPort::default());
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    for index in 0..3 {
        session
            .append_message(user_message(&format!("user {index}")))
            .await
            .unwrap();
        session
            .append_message(AgentMessage::Llm(theway_llm_provider::Message::Assistant(
                assistant(&format!("assistant {index}")),
            )))
            .await
            .unwrap();
    }
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut message = assistant("");
        message.stop_reason = StopReason::Error;
        message.error_message = Some("summary provider failed".into());
        sender.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: message,
        });
        stream
    });
    let harness = harness_with_port(port.clone(), stream_fn, session.clone());
    *harness.compaction_settings.lock() = compact_settings();

    let error = harness.force_compact(None).await.unwrap_err();

    assert!(error.to_string().contains("summary provider failed"));
    let events = port
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ExtensionLifecycleEvent::BeforeCompaction
                    | ExtensionLifecycleEvent::CompactionSucceeded
                    | ExtensionLifecycleEvent::CompactionFailed
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            ExtensionLifecycleEvent::BeforeCompaction,
            ExtensionLifecycleEvent::CompactionFailed,
        ]
    );
    assert!(!session
        .entries()
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry, SessionTreeEntry::Compaction { .. })));
}

struct CapturingAlgorithm {
    messages: Arc<Mutex<Vec<AgentMessage>>>,
}

#[async_trait]
impl CompactAlgorithm for CapturingAlgorithm {
    fn name(&self) -> &str {
        "capture"
    }

    async fn summarize_prefix(
        &self,
        request: &SummarizeRequest<'_>,
    ) -> Result<SummaryOutcome, crate::agent::compaction::compaction::SummarizeError> {
        *self.messages.lock() = request.messages.to_vec();
        Ok(SummaryOutcome {
            summary: "captured".into(),
            usage: Usage::default(),
        })
    }
}

fn durable_entry(origin_sequence: u64, entry: ExtensionDurableEntryPayload) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        abi_major: ExtensionAbiMajor::V2,
        extension_id: "anchor-test".into(),
        state_schema_version: 1,
        origin_sequence,
        entry,
    }
}

#[tokio::test]
async fn compaction_receives_deduplicated_model_context_but_never_private_state() {
    let projection = ExtensionModelContextProjection::rebuild(vec![
        durable_entry(
            1,
            ExtensionDurableEntryPayload::StateMutation {
                key: "private-key".into(),
                mutation: ExtensionStateMutation::Set {
                    value: serde_json::json!("private-secret"),
                },
            },
        ),
        durable_entry(
            2,
            ExtensionDurableEntryPayload::ModelContext {
                context_id: "anchor".into(),
                placement: ExtensionModelContextPlacement::SystemPromptSection,
                content: serde_json::json!("old-anchor"),
            },
        ),
        durable_entry(
            3,
            ExtensionDurableEntryPayload::ModelContext {
                context_id: "anchor".into(),
                placement: ExtensionModelContextPlacement::SystemPromptSection,
                content: serde_json::json!("latest-anchor"),
            },
        ),
        durable_entry(
            4,
            ExtensionDurableEntryPayload::ModelContext {
                context_id: "message".into(),
                placement: ExtensionModelContextPlacement::Message,
                content: serde_json::to_value(user_message("persistent-message")).unwrap(),
            },
        ),
    ])
    .unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut registry = CompactAlgorithmRegistry::new();
    registry.register(Arc::new(CapturingAlgorithm {
        messages: Arc::clone(&captured),
    }));
    let port = Arc::new(RecordingPort::default());
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    let mut options = AgentHarnessOptions::new(faux_model(), session);
    options.observation_context.session_id = Some("session-compaction-context".into());
    options.runtime_extension_cwd = "/workspace".into();
    options.runtime_extensions = port;
    options.runtime_extension_model_context = projection;
    options.stream_fn = Some(success_stream(Arc::new(AtomicUsize::new(0))));
    options.compaction = CompactionSettings {
        algorithm: "capture".into(),
        ..compact_settings()
    };
    options.compact_algorithms = Arc::new(registry);
    let harness = AgentHarness::new(options);
    populate(&harness).await;

    assert!(harness.force_compact(None).await.unwrap());

    let serialized = serde_json::to_string(&*captured.lock()).unwrap();
    assert_eq!(serialized.matches("latest-anchor").count(), 1);
    assert_eq!(serialized.matches("persistent-message").count(), 1);
    assert!(!serialized.contains("old-anchor"));
    assert!(!serialized.contains("private-secret"));
}
