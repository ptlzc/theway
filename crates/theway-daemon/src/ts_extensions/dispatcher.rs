use serde_json::Value;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionCancellationContext, ExtensionEventContext, ExtensionEventEnvelope,
    ExtensionLifecycleEvent, ExtensionScopeIds,
};
use theway_core::agent::runtime_extensions::RuntimeExtensionInvocation;

pub(super) fn envelope(
    extension_id: &str,
    session_id: &str,
    cwd: &str,
    sequence: u64,
    event: ExtensionLifecycleEvent,
    payload: Value,
) -> ExtensionEventEnvelope {
    ExtensionEventEnvelope {
        abi_major: ExtensionAbiMajor::V2,
        event,
        context: ExtensionEventContext {
            extension_id: extension_id.to_string(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            sequence,
            scope: ExtensionScopeIds::default(),
            model: None,
            has_interactive_client: false,
            cancellation: ExtensionCancellationContext::default(),
        },
        payload,
    }
}

pub(super) fn runtime_envelope(
    extension_id: &str,
    invocation: &RuntimeExtensionInvocation,
) -> ExtensionEventEnvelope {
    let context = invocation.context();
    ExtensionEventEnvelope {
        abi_major: ExtensionAbiMajor::V2,
        event: invocation.event(),
        context: ExtensionEventContext {
            extension_id: extension_id.to_string(),
            session_id: context.session_id.clone(),
            cwd: context.cwd.clone(),
            sequence: context.sequence,
            scope: context.scope.clone(),
            model: context.model.clone(),
            has_interactive_client: context.has_interactive_client,
            cancellation: ExtensionCancellationContext {
                cancelled: context.cancelled,
                deadline_unix_ms: context.deadline_unix_ms,
            },
        },
        payload: invocation.payload().clone(),
    }
}
