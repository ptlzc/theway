use serde_json::json;
use theway_contract::extension::{
    ExtensionCancellationContext, ExtensionEventContext, ExtensionEventEnvelope,
    ExtensionLifecycleEvent, ExtensionModelRef, ExtensionScopeIds,
};

#[test]
fn lifecycle_envelope_round_trips_without_runtime_types() {
    let envelope = ExtensionEventEnvelope {
        event: ExtensionLifecycleEvent::ToolExecutionStart,
        context: ExtensionEventContext {
            extension_id: "example-extension".into(),
            session_id: "0198c000-0000-7000-8000-000000000001".into(),
            cwd: "/workspace".into(),
            sequence: 42,
            scope: ExtensionScopeIds {
                run_id: Some("run-1".into()),
                turn_id: Some("turn-2".into()),
                request_id: Some("request-3".into()),
                message_id: Some("message-4".into()),
                tool_call_id: Some("tool-5".into()),
            },
            model: Some(ExtensionModelRef {
                provider: "openai".into(),
                model: "gpt-5".into(),
            }),
            has_interactive_client: true,
            cancellation: ExtensionCancellationContext {
                cancelled: false,
                deadline_unix_ms: Some(1_787_200_000_000),
            },
        },
        payload: json!({"toolName": "bash"}),
    };

    let encoded = serde_json::to_value(&envelope).unwrap();
    let decoded: ExtensionEventEnvelope = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, envelope);
    assert_eq!(encoded["event"], "tool_execution_start");
    assert_eq!(encoded["context"]["scope"]["toolCallId"], "tool-5");
}

#[test]
fn lifecycle_context_defaults_optional_fields_without_sensitive_placeholders() {
    let decoded: ExtensionEventEnvelope = serde_json::from_value(json!({
        "event": "session_start",
        "context": {
            "extensionId": "example-extension",
            "sessionId": "session-1",
            "cwd": "/workspace",
            "sequence": 1
        },
        "payload": {"reason": "new"}
    }))
    .unwrap();

    assert_eq!(decoded.context.scope, ExtensionScopeIds::default());
    assert_eq!(
        decoded.context.cancellation,
        ExtensionCancellationContext::default()
    );
    assert!(decoded.context.model.is_none());
    assert!(!decoded.context.has_interactive_client);
}

#[test]
fn lifecycle_names_cover_success_and_failure_boundaries() {
    for (name, expected) in [
        (
            "provider_request_failed",
            ExtensionLifecycleEvent::ProviderRequestFailed,
        ),
        ("run_settled", ExtensionLifecycleEvent::RunSettled),
        (
            "compaction_failed",
            ExtensionLifecycleEvent::CompactionFailed,
        ),
        ("extension_unload", ExtensionLifecycleEvent::ExtensionUnload),
    ] {
        let decoded: ExtensionLifecycleEvent =
            serde_json::from_value(serde_json::Value::String(name.into())).unwrap();
        assert_eq!(decoded, expected);
    }
}

#[test]
fn new_lifecycle_events_round_trip_snake_case() {
    for (name, expected) in [
        ("session_resume", ExtensionLifecycleEvent::SessionResume),
        ("approval_request", ExtensionLifecycleEvent::ApprovalRequest),
        (
            "approval_resolved",
            ExtensionLifecycleEvent::ApprovalResolved,
        ),
        ("file_write", ExtensionLifecycleEvent::FileWrite),
        ("sandbox_exec", ExtensionLifecycleEvent::SandboxExec),
        (
            "notification_send",
            ExtensionLifecycleEvent::NotificationSend,
        ),
        ("agent_status", ExtensionLifecycleEvent::AgentStatus),
        ("custom", ExtensionLifecycleEvent::Custom),
    ] {
        let decoded: ExtensionLifecycleEvent =
            serde_json::from_value(serde_json::Value::String(name.into())).unwrap();
        assert_eq!(decoded, expected);
        let encoded = serde_json::to_string(&expected).unwrap();
        assert_eq!(encoded, format!("\"{name}\""));
    }
}

#[test]
fn public_name_round_trips_for_every_variant() {
    let all = [
        ExtensionLifecycleEvent::ExtensionLoad,
        ExtensionLifecycleEvent::SessionStart,
        ExtensionLifecycleEvent::Input,
        ExtensionLifecycleEvent::BeforeSessionSwitch,
        ExtensionLifecycleEvent::SessionSwitched,
        ExtensionLifecycleEvent::BeforeSessionFork,
        ExtensionLifecycleEvent::SessionForked,
        ExtensionLifecycleEvent::BeforeModelSelection,
        ExtensionLifecycleEvent::ModelSelected,
        ExtensionLifecycleEvent::BeforeRun,
        ExtensionLifecycleEvent::RunStarted,
        ExtensionLifecycleEvent::TurnStarted,
        ExtensionLifecycleEvent::Context,
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
        ExtensionLifecycleEvent::BeforeProviderRequestRaw,
        ExtensionLifecycleEvent::ProviderResponse,
        ExtensionLifecycleEvent::ProviderRequestFailed,
        ExtensionLifecycleEvent::MessageStart,
        ExtensionLifecycleEvent::MessageUpdate,
        ExtensionLifecycleEvent::MessageEnd,
        ExtensionLifecycleEvent::ToolCall,
        ExtensionLifecycleEvent::ToolExecutionStart,
        ExtensionLifecycleEvent::ToolExecutionUpdate,
        ExtensionLifecycleEvent::ToolExecutionEnd,
        ExtensionLifecycleEvent::ToolResult,
        ExtensionLifecycleEvent::TurnCompleted,
        ExtensionLifecycleEvent::RunEnded,
        ExtensionLifecycleEvent::RunError,
        ExtensionLifecycleEvent::RunSettled,
        ExtensionLifecycleEvent::BeforeCompaction,
        ExtensionLifecycleEvent::CompactionSucceeded,
        ExtensionLifecycleEvent::CompactionFailed,
        ExtensionLifecycleEvent::SessionShutdown,
        ExtensionLifecycleEvent::ExtensionUnload,
        ExtensionLifecycleEvent::SessionResume,
        ExtensionLifecycleEvent::ApprovalRequest,
        ExtensionLifecycleEvent::ApprovalResolved,
        ExtensionLifecycleEvent::FileWrite,
        ExtensionLifecycleEvent::SandboxExec,
        ExtensionLifecycleEvent::NotificationSend,
        ExtensionLifecycleEvent::AgentStatus,
        ExtensionLifecycleEvent::Custom,
    ];
    let mut public_names = std::collections::BTreeSet::new();
    for event in all {
        let public = event.public_name();
        assert!(
            public_names.insert(public.to_string()),
            "duplicate public name {public}"
        );
        assert_eq!(
            ExtensionLifecycleEvent::from_public_name(public),
            Some(event),
            "round-trip failed for {public}"
        );
        // Internal snake_case names remain subscribed as aliases.
        let internal = serde_json::to_value(event).unwrap();
        let internal_name = internal.as_str().unwrap();
        assert_eq!(
            ExtensionLifecycleEvent::from_public_name(internal_name),
            Some(event),
            "internal alias round-trip failed for {internal_name}"
        );
    }
}

#[test]
fn public_name_rejects_unknown_events() {
    assert_eq!(
        ExtensionLifecycleEvent::from_public_name("does/not/exist"),
        None
    );
}
