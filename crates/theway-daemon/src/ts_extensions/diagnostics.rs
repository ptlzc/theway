use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;
use theway_contract::extension::{
    ExtensionDiagnostic, ExtensionDiagnosticCode, ExtensionDiagnosticSeverity,
    ExtensionLifecycleEvent,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmittedDiagnostic {
    code: ExtensionDiagnosticCode,
    severity: ExtensionDiagnosticSeverity,
    message: String,
    #[serde(default)]
    details: BTreeMap<String, Value>,
    #[serde(default)]
    redacted_fields: BTreeSet<String>,
}

pub(super) fn emitted(
    extension_id: impl Into<String>,
    session_id: impl Into<String>,
    event: ExtensionLifecycleEvent,
    sequence: u64,
    payload: Value,
) -> Result<ExtensionDiagnostic, String> {
    let emitted: EmittedDiagnostic = serde_json::from_value(payload)
        .map_err(|error| format!("emitted extension diagnostic is invalid: {error}"))?;
    if emitted.message.trim().is_empty() || emitted.message.len() > 16 * 1024 {
        return Err("emitted extension diagnostic message must contain 1-16384 bytes".into());
    }
    let mut diagnostic = ExtensionDiagnostic::new(
        extension_id,
        emitted.code,
        emitted.severity,
        emitted.message,
    );
    diagnostic.session_id = Some(session_id.into());
    diagnostic.event = Some(event);
    diagnostic.sequence = Some(sequence);
    diagnostic.details = emitted.details;
    diagnostic.redacted_fields = emitted.redacted_fields;
    for field in &diagnostic.redacted_fields {
        diagnostic.details.remove(field);
    }
    Ok(diagnostic)
}

pub(super) fn rejected(
    extension_id: impl Into<String>,
    code: ExtensionDiagnosticCode,
    message: impl Into<String>,
) -> ExtensionDiagnostic {
    ExtensionDiagnostic::new(
        extension_id,
        code,
        ExtensionDiagnosticSeverity::Error,
        message,
    )
}

pub(super) fn shadowed(extension_id: impl Into<String>) -> ExtensionDiagnostic {
    ExtensionDiagnostic::new(
        extension_id,
        ExtensionDiagnosticCode::Shadowed,
        ExtensionDiagnosticSeverity::Info,
        "global package is shadowed by the project package with the same id",
    )
}

pub(super) fn blocked(
    extension_id: impl Into<String>,
    code: ExtensionDiagnosticCode,
) -> ExtensionDiagnostic {
    ExtensionDiagnostic::new(
        extension_id,
        code,
        ExtensionDiagnosticSeverity::Warning,
        match code {
            ExtensionDiagnosticCode::TrustRequired => {
                "project extension requires an explicit trust decision"
            }
            _ => "extension permissions are denied by trust policy",
        },
    )
}

pub(super) fn faulted(
    extension_id: impl Into<String>,
    session_id: impl Into<String>,
    message: impl Into<String>,
) -> ExtensionDiagnostic {
    let mut diagnostic = ExtensionDiagnostic::new(
        extension_id,
        ExtensionDiagnosticCode::LoadFailed,
        ExtensionDiagnosticSeverity::Error,
        message,
    );
    diagnostic.session_id = Some(session_id.into());
    diagnostic
}

pub(super) fn hook_failed(
    extension_id: impl Into<String>,
    session_id: impl Into<String>,
    message: impl Into<String>,
) -> ExtensionDiagnostic {
    let mut diagnostic = ExtensionDiagnostic::new(
        extension_id,
        ExtensionDiagnosticCode::HookFailed,
        ExtensionDiagnosticSeverity::Error,
        message,
    );
    diagnostic.session_id = Some(session_id.into());
    diagnostic
}

pub(super) fn invocation(
    extension_id: impl Into<String>,
    session_id: impl Into<String>,
    event: theway_contract::extension::ExtensionLifecycleEvent,
    code: ExtensionDiagnosticCode,
    message: impl Into<String>,
) -> ExtensionDiagnostic {
    let severity = if matches!(
        code,
        ExtensionDiagnosticCode::Cancelled | ExtensionDiagnosticCode::QueueOverflow
    ) {
        ExtensionDiagnosticSeverity::Warning
    } else {
        ExtensionDiagnosticSeverity::Error
    };
    let mut diagnostic = ExtensionDiagnostic::new(extension_id, code, severity, message);
    diagnostic.session_id = Some(session_id.into());
    diagnostic.event = Some(event);
    diagnostic
}

pub(super) fn circuit_opened(
    extension_id: impl Into<String>,
    session_id: impl Into<String>,
) -> ExtensionDiagnostic {
    let mut diagnostic = ExtensionDiagnostic::new(
        extension_id,
        ExtensionDiagnosticCode::CircuitOpened,
        ExtensionDiagnosticSeverity::Error,
        "extension disabled after repeated hook failures",
    );
    diagnostic.session_id = Some(session_id.into());
    diagnostic
}

pub(super) fn registration_rejected(
    extension_id: impl Into<String>,
    session_id: impl Into<String>,
    message: impl Into<String>,
) -> ExtensionDiagnostic {
    let mut diagnostic = ExtensionDiagnostic::new(
        extension_id,
        ExtensionDiagnosticCode::ContractViolation,
        ExtensionDiagnosticSeverity::Warning,
        format!("extension registration rejected: {}", message.into()),
    );
    diagnostic.session_id = Some(session_id.into());
    diagnostic
}
