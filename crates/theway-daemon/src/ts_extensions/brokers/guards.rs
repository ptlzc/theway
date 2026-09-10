//! Enforcement shared by every broker domain: permission checks, the
//! invocation deadline/cancellation wait, and the audit + diagnostic records
//! that keep a denied or failed capability traceable.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionDiagnostic, ExtensionDiagnosticCode,
    ExtensionDiagnosticSeverity, ExtensionPermission,
};

use super::BrokerError;
use super::BrokerRuntime;

impl BrokerRuntime {
    pub(super) fn require(&self, permission: ExtensionPermission) -> Result<(), BrokerError> {
        if self.permissions.contains(&permission) {
            return Ok(());
        }
        self.diagnose(
            ExtensionDiagnosticCode::PermissionDenied,
            "extension attempted an undeclared or ungranted capability operation",
        );
        self.audit(
            audit_operation(&permission),
            ExtensionAuditOutcome::Denied,
            Some(permission),
            None,
            std::iter::empty(),
        );
        Err(BrokerError::new(
            "permission_denied",
            "extension capability is not declared and granted",
        ))
    }

    pub(super) fn async_failure<T>(
        &self,
        operation: ExtensionAuditOperation,
        permission: ExtensionPermission,
        target: &str,
        error: String,
    ) -> Result<T, BrokerError> {
        let cancelled = error == "cancelled";
        self.audit(
            operation,
            if cancelled {
                ExtensionAuditOutcome::Cancelled
            } else {
                ExtensionAuditOutcome::Failed
            },
            Some(permission),
            Some(target),
            std::iter::empty(),
        );
        Err(BrokerError::new(
            if cancelled {
                "cancelled"
            } else {
                "broker_failed"
            },
            if cancelled {
                "broker operation was cancelled"
            } else {
                "broker operation failed"
            },
        ))
    }

    pub(super) fn fail_audited<T>(
        &self,
        operation: ExtensionAuditOperation,
        permission: ExtensionPermission,
        target: &str,
        code: &'static str,
        message: &'static str,
    ) -> Result<T, BrokerError> {
        self.audit(
            operation,
            ExtensionAuditOutcome::Failed,
            Some(permission),
            Some(target),
            std::iter::empty(),
        );
        Err(BrokerError::new(code, message))
    }

    pub(super) fn audit(
        &self,
        operation: ExtensionAuditOperation,
        outcome: ExtensionAuditOutcome,
        capability: Option<ExtensionPermission>,
        target: Option<&str>,
        redacted_fields: impl IntoIterator<Item = String>,
    ) {
        self.services.audit.record(
            self.extension_id.clone(),
            Some(self.session_id.clone()),
            operation,
            outcome,
            capability,
            target,
            redacted_fields,
        );
    }

    pub(super) fn diagnose(&self, code: ExtensionDiagnosticCode, message: &str) {
        let mut diagnostic = ExtensionDiagnostic::new(
            self.extension_id.clone(),
            code,
            ExtensionDiagnosticSeverity::Error,
            message,
        );
        diagnostic.session_id = Some(self.session_id.clone());
        self.services.diagnostics.lock().push(diagnostic);
    }
}

fn audit_operation(permission: &ExtensionPermission) -> ExtensionAuditOperation {
    match permission {
        ExtensionPermission::WorkspaceRead => ExtensionAuditOperation::WorkspaceRead,
        ExtensionPermission::WorkspaceWrite => ExtensionAuditOperation::WorkspaceWrite,
        ExtensionPermission::ProcessSpawn => ExtensionAuditOperation::ProcessSpawn,
        ExtensionPermission::NetworkConnect => ExtensionAuditOperation::NetworkConnect,
        ExtensionPermission::ProviderRaw => ExtensionAuditOperation::ProviderRawRead,
        ExtensionPermission::SecretsRead(_) => ExtensionAuditOperation::SecretRead,
        _ => ExtensionAuditOperation::TrustChanged,
    }
}

pub(super) async fn cancelled(cancellation: Arc<AtomicBool>, deadline: Instant) {
    loop {
        if cancellation.load(Ordering::Acquire) || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
