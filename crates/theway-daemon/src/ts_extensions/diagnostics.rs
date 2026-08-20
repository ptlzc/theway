use theway_contract::extension::{
    ExtensionDiagnostic, ExtensionDiagnosticCode, ExtensionDiagnosticSeverity,
};

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
