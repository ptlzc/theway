//! Engine-neutral runtime-extension ABI contracts.
//!
//! This module owns serialized values shared by the extension host, runtime,
//! persistence, and protocol layers. It contains no discovery, execution,
//! storage, transport, or UI policy.

mod action;
mod audit;
mod contribution;
mod diagnostic;
mod lifecycle;
mod manifest;
mod plugin;
mod state;
mod trust;

pub use lifecycle::{
    ExtensionCancellationContext, ExtensionEventContext, ExtensionEventEnvelope,
    ExtensionLifecycleEvent, ExtensionModelRef, ExtensionScopeIds,
};
pub use manifest::{
    ExtensionManifestError, ExtensionPackageManifest, ExtensionPermission, ExtensionScope,
    ExtensionSourceLayer,
};
pub use plugin::{PluginActionRegistration, ServiceRegistration};
pub use state::{
    ExtensionDurableEntry, ExtensionDurableEntryKind, ExtensionDurableEntryPayload,
    ExtensionModelContextPlacement, ExtensionStateMutation, ExtensionStateValidationError,
};
pub use trust::{
    ExtensionTrustDecision, ExtensionTrustError, ExtensionTrustRecord, ExtensionTrustSubject,
};

pub(crate) fn is_valid_extension_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn first_duplicate<T>(values: &[T]) -> Option<&T>
where
    T: Ord,
{
    let mut ordered: Vec<&T> = values.iter().collect();
    ordered.sort_unstable();
    ordered
        .windows(2)
        .find_map(|pair| (pair[0] == pair[1]).then_some(pair[0]))
}
pub use action::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind, ExtensionDeliveryPolicy,
    ExtensionErrorCode, ExtensionErrorEnvelope, ExtensionGateDecision, ExtensionHookClass,
    ExtensionHookContract, ExtensionHookDeadline, ExtensionHookFailurePolicy,
};
pub use audit::{ExtensionAuditEvent, ExtensionAuditOperation, ExtensionAuditOutcome};
pub use contribution::{
    ExtensionClientContribution, ExtensionClientContributionData, ExtensionCommandDescriptor,
    ExtensionCommandOutcome, ExtensionContributionError, ExtensionNoticeLevel,
};
pub use diagnostic::{
    ExtensionCatalogEntry, ExtensionCatalogStatus, ExtensionDiagnostic, ExtensionDiagnosticCode,
    ExtensionDiagnosticSensitivity, ExtensionDiagnosticSeverity,
};
