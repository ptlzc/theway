use serde_json::json;
use theway_contract::extension::{
    ExtensionDiagnosticCode, ExtensionDiagnosticSeverity, ExtensionLifecycleEvent,
};

use super::super::diagnostics::{
    blocked, circuit_opened, emitted, faulted, hook_failed, invocation, registration_rejected,
    rejected, shadowed,
};

#[test]
fn emitted_diagnostic_builds_structured_diagnostic_and_redacts_fields() {
    let diagnostic = emitted(
        "ext",
        "sess",
        ExtensionLifecycleEvent::Input,
        7,
        json!({
            "code": "hook_failed",
            "severity": "error",
            "message": "boom",
            "details": {"secret": "value", "keep": true},
            "redactedFields": ["secret"]
        }),
    )
    .unwrap();

    assert_eq!(diagnostic.extension_id, "ext");
    assert_eq!(diagnostic.session_id.as_deref(), Some("sess"));
    assert_eq!(diagnostic.event, Some(ExtensionLifecycleEvent::Input));
    assert_eq!(diagnostic.sequence, Some(7));
    assert_eq!(diagnostic.details.get("secret"), None);
    assert_eq!(diagnostic.details.get("keep"), Some(&json!(true)));
    assert!(diagnostic.redacted_fields.contains("secret"));
}

#[test]
fn emitted_diagnostic_rejects_empty_or_oversized_message() {
    let err = emitted("ext", "s", ExtensionLifecycleEvent::Input, 1, json!({
        "code": "hook_failed",
        "severity": "error",
        "message": "   "
    }))
    .unwrap_err();
    assert!(err.contains("1-16384"), "{err}");

    let err = emitted("ext", "s", ExtensionLifecycleEvent::Input, 1, json!({
        "code": "hook_failed",
        "severity": "error",
        "message": "x".repeat(16 * 1024 + 1)
    }))
    .unwrap_err();
    assert!(err.contains("1-16384"), "{err}");
}

#[test]
fn emitted_diagnostic_rejects_invalid_payload() {
    let err = emitted("ext", "s", ExtensionLifecycleEvent::Input, 1, json!({"bad": true}))
        .unwrap_err();
    assert!(err.contains("invalid"), "{err}");
}

#[test]
fn simple_diagnostic_helpers_populate_expected_fields() {
    let rejected = rejected("ext", ExtensionDiagnosticCode::LoadFailed, "bad");
    assert_eq!(rejected.extension_id, "ext");
    assert_eq!(rejected.code, ExtensionDiagnosticCode::LoadFailed);
    assert_eq!(rejected.severity, ExtensionDiagnosticSeverity::Error);

    let shadowed = shadowed("ext");
    assert_eq!(shadowed.code, ExtensionDiagnosticCode::Shadowed);
    assert_eq!(shadowed.severity, ExtensionDiagnosticSeverity::Info);

    let blocked_trust = blocked("ext", ExtensionDiagnosticCode::TrustRequired);
    assert_eq!(blocked_trust.code, ExtensionDiagnosticCode::TrustRequired);
    assert_eq!(blocked_trust.severity, ExtensionDiagnosticSeverity::Warning);
    let blocked_other = blocked("ext", ExtensionDiagnosticCode::PermissionDenied);
    assert_eq!(blocked_other.severity, ExtensionDiagnosticSeverity::Warning);
    assert!(blocked_other.message.contains("denied"));

    let faulted = faulted("ext", "sess", "load");
    assert_eq!(faulted.code, ExtensionDiagnosticCode::LoadFailed);
    assert_eq!(faulted.session_id.as_deref(), Some("sess"));

    let hook_failed = hook_failed("ext", "sess", "hook");
    assert_eq!(hook_failed.code, ExtensionDiagnosticCode::HookFailed);
    assert_eq!(hook_failed.session_id.as_deref(), Some("sess"));
}

#[test]
fn invocation_diagnostic_uses_warning_for_cancelled_and_overflow() {
    let cancelled = invocation(
        "ext",
        "sess",
        ExtensionLifecycleEvent::Input,
        ExtensionDiagnosticCode::Cancelled,
        "stop",
    );
    assert_eq!(cancelled.severity, ExtensionDiagnosticSeverity::Warning);
    assert_eq!(cancelled.event, Some(ExtensionLifecycleEvent::Input));

    let overflow = invocation(
        "ext",
        "sess",
        ExtensionLifecycleEvent::Input,
        ExtensionDiagnosticCode::QueueOverflow,
        "drop",
    );
    assert_eq!(overflow.severity, ExtensionDiagnosticSeverity::Warning);

    let error = invocation(
        "ext",
        "sess",
        ExtensionLifecycleEvent::Input,
        ExtensionDiagnosticCode::HookFailed,
        "bad",
    );
    assert_eq!(error.severity, ExtensionDiagnosticSeverity::Error);
}

#[test]
fn circuit_opened_and_registration_rejected_helpers() {
    let circuit = circuit_opened("ext", "sess");
    assert_eq!(circuit.code, ExtensionDiagnosticCode::CircuitOpened);
    assert_eq!(circuit.session_id.as_deref(), Some("sess"));

    let rejected = registration_rejected("ext", "sess", "nope");
    assert_eq!(rejected.code, ExtensionDiagnosticCode::ContractViolation);
    assert_eq!(rejected.severity, ExtensionDiagnosticSeverity::Warning);
    assert!(rejected.message.contains("nope"));
}
