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
