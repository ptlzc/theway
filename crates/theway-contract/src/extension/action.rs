use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ExtensionAbiMajor, ExtensionDurableEntry, ExtensionDurableEntryKind, ExtensionLifecycleEvent,
};

/// Semantic class assigned to every public hook.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionHookClass {
    Observe,
    Transform,
    Gate,
    Register,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionHookDeadline {
    Fast,
    Standard,
    Long,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDeliveryPolicy {
    Inline,
    BoundedCoalescing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionHookFailurePolicy {
    Continue,
    KeepLastValue,
    Deny,
    RejectRegistration,
}

/// Stable action categories. Complex payloads stay JSON-compatible at the ABI
/// boundary and are decoded into domain-specific DTOs before application.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionActionKind {
    ReplaceInput,
    PatchRunContext,
    ReplaceContext,
    ReplaceModelRequest,
    ReplaceProviderHeaders,
    ReplaceProviderPayload,
    ReplaceMessage,
    ReplaceToolResult,
    SetState,
    DeleteState,
    AppendCustomEvent,
    AppendModelContext,
    EmitCommandOutcome,
    EnqueueFollowUp,
    EmitDiagnostic,
    RegisterEffect,
    DisposeEffect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionAction {
    pub kind: ExtensionActionKind,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionGateDecision {
    Abstain,
    Allow,
    Deny { code: String, message: String },
    Cancel { code: String, message: String },
}

/// Complete return value from one hook invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionActionBatch {
    pub abi_major: ExtensionAbiMajor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ExtensionGateDecision>,
    #[serde(default)]
    pub actions: Vec<ExtensionAction>,
}

/// Public metadata for one event/class pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionHookContract {
    pub event: ExtensionLifecycleEvent,
    pub class: ExtensionHookClass,
    pub allowed_actions: Vec<ExtensionActionKind>,
    pub deadline: ExtensionHookDeadline,
    pub delivery: ExtensionDeliveryPolicy,
    pub failure: ExtensionHookFailurePolicy,
}

impl ExtensionHookContract {
    pub fn for_hook(
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
    ) -> Result<Self, ExtensionErrorEnvelope> {
        let allowed_actions = match class {
            ExtensionHookClass::Observe => Vec::new(),
            ExtensionHookClass::Transform => transform_actions(event)
                .ok_or_else(|| ExtensionErrorEnvelope::invalid_hook(event, class))?,
            ExtensionHookClass::Gate => {
                if !is_gate_event(event) {
                    return Err(ExtensionErrorEnvelope::invalid_hook(event, class));
                }
                persistent_actions()
            }
            ExtensionHookClass::Register => {
                if event != ExtensionLifecycleEvent::ExtensionLoad {
                    return Err(ExtensionErrorEnvelope::invalid_hook(event, class));
                }
                vec![
                    ExtensionActionKind::RegisterEffect,
                    ExtensionActionKind::DisposeEffect,
                    ExtensionActionKind::EmitDiagnostic,
                ]
            }
        };
        let deadline = if matches!(
            event,
            ExtensionLifecycleEvent::MessageUpdate | ExtensionLifecycleEvent::ToolExecutionUpdate
        ) {
            ExtensionHookDeadline::Fast
        } else if matches!(
            event,
            ExtensionLifecycleEvent::ExtensionLoad
                | ExtensionLifecycleEvent::SessionStart
                | ExtensionLifecycleEvent::SessionShutdown
                | ExtensionLifecycleEvent::ExtensionUnload
        ) {
            ExtensionHookDeadline::Long
        } else {
            ExtensionHookDeadline::Standard
        };
        let delivery = if class == ExtensionHookClass::Observe
            && matches!(
                event,
                ExtensionLifecycleEvent::MessageUpdate
                    | ExtensionLifecycleEvent::ToolExecutionUpdate
            ) {
            ExtensionDeliveryPolicy::BoundedCoalescing
        } else {
            ExtensionDeliveryPolicy::Inline
        };
        let failure = match class {
            ExtensionHookClass::Observe => ExtensionHookFailurePolicy::Continue,
            ExtensionHookClass::Transform => ExtensionHookFailurePolicy::KeepLastValue,
            ExtensionHookClass::Gate => ExtensionHookFailurePolicy::Deny,
            ExtensionHookClass::Register => ExtensionHookFailurePolicy::RejectRegistration,
        };
        Ok(Self {
            event,
            class,
            allowed_actions,
            deadline,
            delivery,
            failure,
        })
    }

    pub fn validate_result(
        &self,
        result: &ExtensionActionBatch,
    ) -> Result<(), ExtensionErrorEnvelope> {
        if !result.abi_major.is_supported() {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::AbiMismatch,
                format!("unsupported extension ABI major {}", result.abi_major.0),
            ));
        }
        if self.class != ExtensionHookClass::Gate && result.decision.is_some() {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::ContractViolation,
                "only a gate hook may return a gate decision",
            ));
        }
        if self.class == ExtensionHookClass::Observe && !result.actions.is_empty() {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::ContractViolation,
                "observe hooks cannot return actions",
            ));
        }

        let allowed: BTreeSet<_> = self.allowed_actions.iter().copied().collect();
        let mut singleton_actions = BTreeSet::new();
        for action in &result.actions {
            if !allowed.contains(&action.kind) {
                return Err(ExtensionErrorEnvelope::new(
                    ExtensionErrorCode::InvalidAction,
                    format!(
                        "action {:?} is not allowed for {:?}/{:?}",
                        action.kind, self.event, self.class
                    ),
                ));
            }
            if !action.payload.is_object() {
                return Err(ExtensionErrorEnvelope::new(
                    ExtensionErrorCode::InvalidPayload,
                    format!("action {:?} payload must be an object", action.kind),
                ));
            }
            validate_durable_action(action)?;
            if is_singleton_action(action.kind) && !singleton_actions.insert(action.kind) {
                return Err(ExtensionErrorEnvelope::new(
                    ExtensionErrorCode::InvalidAction,
                    format!("action {:?} may appear only once", action.kind),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionErrorCode {
    AbiMismatch,
    InvalidHook,
    ContractViolation,
    InvalidAction,
    InvalidPayload,
    PermissionDenied,
    Timeout,
    Cancelled,
    ReentrantCall,
    ResourceLimit,
    PersistenceFailed,
    Conflict,
    UnsupportedFormat,
    StateMigrationFailed,
    CircuitOpen,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionErrorEnvelope {
    pub abi_major: ExtensionAbiMajor,
    pub code: ExtensionErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ExtensionErrorEnvelope {
    pub fn new(code: ExtensionErrorCode, message: impl Into<String>) -> Self {
        Self {
            abi_major: ExtensionAbiMajor::V2,
            code,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    fn invalid_hook(event: ExtensionLifecycleEvent, class: ExtensionHookClass) -> Self {
        Self::new(
            ExtensionErrorCode::InvalidHook,
            format!("event {event:?} does not support hook class {class:?}"),
        )
    }
}

fn transform_actions(event: ExtensionLifecycleEvent) -> Option<Vec<ExtensionActionKind>> {
    let primary = match event {
        ExtensionLifecycleEvent::Input => ExtensionActionKind::ReplaceInput,
        ExtensionLifecycleEvent::BeforeRun => ExtensionActionKind::PatchRunContext,
        ExtensionLifecycleEvent::Context => ExtensionActionKind::ReplaceContext,
        ExtensionLifecycleEvent::BeforeModelRequest => ExtensionActionKind::ReplaceModelRequest,
        ExtensionLifecycleEvent::BeforeProviderRequestHeaders => {
            ExtensionActionKind::ReplaceProviderHeaders
        }
        ExtensionLifecycleEvent::BeforeProviderRequestRaw => {
            ExtensionActionKind::ReplaceProviderPayload
        }
        ExtensionLifecycleEvent::MessageEnd => ExtensionActionKind::ReplaceMessage,
        ExtensionLifecycleEvent::ToolResult => ExtensionActionKind::ReplaceToolResult,
        _ => return None,
    };
    let mut actions = vec![primary];
    if event == ExtensionLifecycleEvent::Input {
        actions.push(ExtensionActionKind::EmitCommandOutcome);
    }
    actions.extend(persistent_actions());
    Some(actions)
}

fn persistent_actions() -> Vec<ExtensionActionKind> {
    vec![
        ExtensionActionKind::SetState,
        ExtensionActionKind::DeleteState,
        ExtensionActionKind::AppendCustomEvent,
        ExtensionActionKind::AppendModelContext,
        ExtensionActionKind::EnqueueFollowUp,
        ExtensionActionKind::EmitDiagnostic,
    ]
}

fn is_gate_event(event: ExtensionLifecycleEvent) -> bool {
    matches!(
        event,
        ExtensionLifecycleEvent::BeforeSessionSwitch
            | ExtensionLifecycleEvent::BeforeSessionFork
            | ExtensionLifecycleEvent::BeforeModelSelection
            | ExtensionLifecycleEvent::ToolCall
            | ExtensionLifecycleEvent::BeforeCompaction
    )
}

fn is_singleton_action(kind: ExtensionActionKind) -> bool {
    matches!(
        kind,
        ExtensionActionKind::ReplaceInput
            | ExtensionActionKind::PatchRunContext
            | ExtensionActionKind::ReplaceContext
            | ExtensionActionKind::ReplaceModelRequest
            | ExtensionActionKind::ReplaceProviderHeaders
            | ExtensionActionKind::ReplaceProviderPayload
            | ExtensionActionKind::ReplaceMessage
            | ExtensionActionKind::ReplaceToolResult
            | ExtensionActionKind::EmitCommandOutcome
    )
}

fn validate_durable_action(action: &ExtensionAction) -> Result<(), ExtensionErrorEnvelope> {
    let expected_kind = match action.kind {
        ExtensionActionKind::SetState | ExtensionActionKind::DeleteState => {
            Some(ExtensionDurableEntryKind::StateMutation)
        }
        ExtensionActionKind::AppendCustomEvent => Some(ExtensionDurableEntryKind::CustomEvent),
        ExtensionActionKind::AppendModelContext => Some(ExtensionDurableEntryKind::ModelContext),
        _ => None,
    };
    let Some(expected_kind) = expected_kind else {
        return Ok(());
    };
    let entry: ExtensionDurableEntry =
        serde_json::from_value(action.payload.clone()).map_err(|error| {
            ExtensionErrorEnvelope::new(
                ExtensionErrorCode::InvalidPayload,
                format!("invalid durable extension action payload: {error}"),
            )
        })?;
    entry.validate().map_err(|error| {
        ExtensionErrorEnvelope::new(ExtensionErrorCode::InvalidPayload, error.to_string())
    })?;
    if entry.entry.kind() != expected_kind {
        return Err(ExtensionErrorEnvelope::new(
            ExtensionErrorCode::InvalidPayload,
            format!(
                "action {:?} requires durable entry kind {:?}",
                action.kind, expected_kind
            ),
        ));
    }
    match (&action.kind, &entry.entry) {
        (
            ExtensionActionKind::SetState,
            super::ExtensionDurableEntryPayload::StateMutation {
                mutation: super::ExtensionStateMutation::Set { .. },
                ..
            },
        )
        | (
            ExtensionActionKind::DeleteState,
            super::ExtensionDurableEntryPayload::StateMutation {
                mutation: super::ExtensionStateMutation::Delete,
                ..
            },
        )
        | (ExtensionActionKind::AppendCustomEvent, _)
        | (ExtensionActionKind::AppendModelContext, _) => Ok(()),
        _ => Err(ExtensionErrorEnvelope::new(
            ExtensionErrorCode::InvalidPayload,
            format!(
                "action {:?} does not match the durable state operation",
                action.kind
            ),
        )),
    }
}
