//! Engine-independent runtime-extension ports owned by core lifecycle seams.
//!
//! The embedding runtime implements these ports and translates an invocation to
//! its extension engine. Core validates every returned ABI action batch before
//! exposing a class-specific result to the lifecycle call site.

mod compaction;
mod context;
mod message;
mod request;
mod run;
mod scope;
mod session;
mod state;
mod tool;

use async_trait::async_trait;
use serde_json::Value;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionAction, ExtensionActionBatch, ExtensionErrorCode,
    ExtensionErrorEnvelope, ExtensionGateDecision, ExtensionHookClass, ExtensionHookContract,
    ExtensionLifecycleEvent, ExtensionModelRef, ExtensionScopeIds,
};

pub use compaction::RuntimeCompactionExtensionPort;
pub use context::{
    ExtensionModelContextItem, ExtensionModelContextProjection,
    ExtensionModelContextProjectionError,
};
pub use message::RuntimeMessageExtensionPort;
pub use request::RuntimeRequestExtensionPort;
pub use run::RuntimeRunExtensionPort;
pub use scope::{RuntimeExtensionScopeAllocator, RuntimeExtensionScopeKind, ScopeAllocationError};
pub use session::RuntimeSessionExtensionPort;
pub use state::{
    NoopSessionExtensionStatePort, PersistentSessionExtensionStatePort, SessionExtensionStateError,
    SessionExtensionStatePort,
};
pub use tool::RuntimeToolExtensionPort;

pub type RawRuntimeExtensionResult = Result<ExtensionActionBatch, ExtensionErrorEnvelope>;
pub type RuntimeExtensionResult = Result<ValidatedRuntimeExtensionResult, ExtensionErrorEnvelope>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeExtensionContext {
    pub session_id: String,
    pub cwd: String,
    pub sequence: u64,
    pub scope: ExtensionScopeIds,
    pub model: Option<ExtensionModelRef>,
    pub has_interactive_client: bool,
    pub cancelled: bool,
    pub deadline_unix_ms: Option<u64>,
}

impl RuntimeExtensionContext {
    pub fn new(session_id: impl Into<String>, cwd: impl Into<String>, sequence: u64) -> Self {
        Self {
            session_id: session_id.into(),
            cwd: cwd.into(),
            sequence,
            scope: ExtensionScopeIds::default(),
            model: None,
            has_interactive_client: false,
            cancelled: false,
            deadline_unix_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeExtensionInvocation {
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
    context: RuntimeExtensionContext,
    payload: Value,
}

impl RuntimeExtensionInvocation {
    pub fn new(
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
        context: RuntimeExtensionContext,
        payload: Value,
    ) -> Result<Self, ExtensionErrorEnvelope> {
        ExtensionHookContract::for_hook(event, class)?;
        if context.session_id.trim().is_empty() || context.cwd.trim().is_empty() {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::InvalidPayload,
                "runtime extension context requires session_id and cwd",
            ));
        }
        if context.sequence == 0 {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::InvalidPayload,
                "runtime extension lifecycle sequence must be greater than zero",
            ));
        }
        if !payload.is_object() {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::InvalidPayload,
                "runtime extension event payload must be an object",
            ));
        }
        Ok(Self {
            event,
            class,
            context,
            payload,
        })
    }

    pub const fn event(&self) -> ExtensionLifecycleEvent {
        self.event
    }

    pub const fn class(&self) -> ExtensionHookClass {
        self.class
    }

    pub fn context(&self) -> &RuntimeExtensionContext {
        &self.context
    }

    pub fn payload(&self) -> &Value {
        &self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedObserveResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedTransformResult {
    event: ExtensionLifecycleEvent,
    actions: Vec<ExtensionAction>,
}

impl ValidatedTransformResult {
    pub const fn event(&self) -> ExtensionLifecycleEvent {
        self.event
    }

    pub fn actions(&self) -> &[ExtensionAction] {
        &self.actions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedGateResult {
    event: ExtensionLifecycleEvent,
    decision: ExtensionGateDecision,
    actions: Vec<ExtensionAction>,
}

impl ValidatedGateResult {
    pub const fn event(&self) -> ExtensionLifecycleEvent {
        self.event
    }

    pub fn decision(&self) -> &ExtensionGateDecision {
        &self.decision
    }

    pub fn actions(&self) -> &[ExtensionAction] {
        &self.actions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedRegisterResult {
    actions: Vec<ExtensionAction>,
}

impl ValidatedRegisterResult {
    pub fn actions(&self) -> &[ExtensionAction] {
        &self.actions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidatedRuntimeExtensionResult {
    Observe(ValidatedObserveResult),
    Transform(ValidatedTransformResult),
    Gate(ValidatedGateResult),
    Register(ValidatedRegisterResult),
}

pub trait RuntimeExtensionPort:
    RuntimeSessionExtensionPort
    + RuntimeRunExtensionPort
    + RuntimeRequestExtensionPort
    + RuntimeMessageExtensionPort
    + RuntimeToolExtensionPort
    + RuntimeCompactionExtensionPort
    + Send
    + Sync
{
}

impl<T> RuntimeExtensionPort for T where
    T: RuntimeSessionExtensionPort
        + RuntimeRunExtensionPort
        + RuntimeRequestExtensionPort
        + RuntimeMessageExtensionPort
        + RuntimeToolExtensionPort
        + RuntimeCompactionExtensionPort
        + Send
        + Sync
{
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeExtensionDomain {
    Session,
    Run,
    Request,
    Message,
    Tool,
    Compaction,
}

fn validate_domain_event(
    domain: RuntimeExtensionDomain,
    invocation: &RuntimeExtensionInvocation,
) -> Result<(), ExtensionErrorEnvelope> {
    let event = invocation.event();
    let valid = match domain {
        RuntimeExtensionDomain::Session => matches!(
            event,
            ExtensionLifecycleEvent::SessionStart
                | ExtensionLifecycleEvent::BeforeSessionSwitch
                | ExtensionLifecycleEvent::SessionSwitched
                | ExtensionLifecycleEvent::BeforeSessionFork
                | ExtensionLifecycleEvent::SessionForked
                | ExtensionLifecycleEvent::SessionShutdown
        ),
        RuntimeExtensionDomain::Run => matches!(
            event,
            ExtensionLifecycleEvent::BeforeRun
                | ExtensionLifecycleEvent::RunStarted
                | ExtensionLifecycleEvent::TurnStarted
                | ExtensionLifecycleEvent::TurnCompleted
                | ExtensionLifecycleEvent::RunEnded
                | ExtensionLifecycleEvent::RunError
                | ExtensionLifecycleEvent::RunSettled
        ),
        RuntimeExtensionDomain::Request => matches!(
            event,
            ExtensionLifecycleEvent::Input
                | ExtensionLifecycleEvent::BeforeModelSelection
                | ExtensionLifecycleEvent::ModelSelected
                | ExtensionLifecycleEvent::Context
                | ExtensionLifecycleEvent::BeforeModelRequest
        ),
        RuntimeExtensionDomain::Message => matches!(
            event,
            ExtensionLifecycleEvent::MessageStart
                | ExtensionLifecycleEvent::MessageUpdate
                | ExtensionLifecycleEvent::MessageEnd
        ),
        RuntimeExtensionDomain::Tool => matches!(
            event,
            ExtensionLifecycleEvent::ToolCall
                | ExtensionLifecycleEvent::ToolExecutionStart
                | ExtensionLifecycleEvent::ToolExecutionUpdate
                | ExtensionLifecycleEvent::ToolExecutionEnd
                | ExtensionLifecycleEvent::ToolResult
        ),
        RuntimeExtensionDomain::Compaction => matches!(
            event,
            ExtensionLifecycleEvent::BeforeCompaction
                | ExtensionLifecycleEvent::CompactionSucceeded
                | ExtensionLifecycleEvent::CompactionFailed
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(ExtensionErrorEnvelope::new(
            ExtensionErrorCode::InvalidHook,
            format!("event {event:?} does not belong to the {domain:?} core extension port"),
        ))
    }
}

fn validate_hook_result(
    invocation: &RuntimeExtensionInvocation,
    result: ExtensionActionBatch,
) -> RuntimeExtensionResult {
    let contract = ExtensionHookContract::for_hook(invocation.event, invocation.class)?;
    contract.validate_result(&result)?;
    Ok(match invocation.class {
        ExtensionHookClass::Observe => {
            ValidatedRuntimeExtensionResult::Observe(ValidatedObserveResult)
        }
        ExtensionHookClass::Transform => {
            ValidatedRuntimeExtensionResult::Transform(ValidatedTransformResult {
                event: invocation.event,
                actions: result.actions,
            })
        }
        ExtensionHookClass::Gate => ValidatedRuntimeExtensionResult::Gate(ValidatedGateResult {
            event: invocation.event,
            decision: result.decision.unwrap_or(ExtensionGateDecision::Abstain),
            actions: result.actions,
        }),
        ExtensionHookClass::Register => {
            ValidatedRuntimeExtensionResult::Register(ValidatedRegisterResult {
                actions: result.actions,
            })
        }
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopRuntimeExtensionPort;

fn empty_action_batch() -> ExtensionActionBatch {
    ExtensionActionBatch {
        abi_major: ExtensionAbiMajor::V2,
        decision: None,
        actions: Vec::new(),
    }
}

macro_rules! impl_noop_domain {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for NoopRuntimeExtensionPort {
            async fn $method(
                &self,
                _invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                Ok(empty_action_batch())
            }
        }
    };
}

impl_noop_domain!(RuntimeSessionExtensionPort, invoke_session);
impl_noop_domain!(RuntimeRunExtensionPort, invoke_run);
impl_noop_domain!(RuntimeRequestExtensionPort, invoke_request);
impl_noop_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_noop_domain!(RuntimeToolExtensionPort, invoke_tool);
impl_noop_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/runtime_extensions");
