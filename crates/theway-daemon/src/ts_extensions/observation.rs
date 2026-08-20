use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use theway_contract::extension::{
    ExtensionActionBatch, ExtensionCatalogStatus, ExtensionDiagnostic, ExtensionDiagnosticCode,
    ExtensionEventEnvelope,
};

use super::catalog::PackageCatalog;
use super::diagnostics;
use super::dispatcher::{HookRegistration, RuntimeExtensionHostConfig};
use super::effects::{EffectOwner, InstanceHealth};
use super::engine::{EngineInstanceKey, EngineInvocationErrorKind, QuickJsEnginePool};
use super::registration_runtime::RegistrationRuntime;

pub(super) struct ObservationJob {
    pub envelope: ExtensionEventEnvelope,
    pub cancellation: Arc<AtomicBool>,
}

struct QueueState {
    in_flight: bool,
    pending: VecDeque<ObservationJob>,
    dropped: usize,
}

pub(super) struct ObservationQueue {
    capacity: usize,
    state: parking_lot::Mutex<QueueState>,
}

impl ObservationQueue {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: parking_lot::Mutex::new(QueueState {
                in_flight: false,
                pending: VecDeque::new(),
                dropped: 0,
            }),
        }
    }

    /// Returns the first job only when the caller must start a drain task.
    pub(super) fn enqueue(&self, job: ObservationJob) -> Option<ObservationJob> {
        let mut state = self.state.lock();
        if !state.in_flight {
            state.in_flight = true;
            return Some(job);
        }
        if state.pending.len() == self.capacity {
            state.pending.pop_back();
            state.dropped = state.dropped.saturating_add(1);
        }
        state.pending.push_back(job);
        None
    }

    fn next(&self) -> Option<(ObservationJob, usize)> {
        let mut state = self.state.lock();
        let Some(job) = state.pending.pop_front() else {
            state.in_flight = false;
            return None;
        };
        let dropped = std::mem::take(&mut state.dropped);
        Some((job, dropped))
    }
}

#[derive(Clone)]
pub(super) struct ObservationDispatch {
    pub extension_id: String,
    pub session_id: String,
    pub key: EngineInstanceKey,
    pub registration: HookRegistration,
    pub engine: QuickJsEnginePool,
    pub config: RuntimeExtensionHostConfig,
    pub health: Arc<InstanceHealth>,
    pub diagnostics: Arc<parking_lot::Mutex<Vec<ExtensionDiagnostic>>>,
    pub catalog: Arc<parking_lot::RwLock<PackageCatalog>>,
    pub registration_runtime: RegistrationRuntime,
}

impl ObservationDispatch {
    pub(super) fn spawn(self, queue: Arc<ObservationQueue>, first: ObservationJob) {
        tokio::spawn(async move {
            let mut current = Some((first, 0));
            while let Some((job, dropped)) = current {
                if dropped != 0 {
                    self.diagnostics.lock().push(diagnostics::invocation(
                        self.extension_id.clone(),
                        self.session_id.clone(),
                        self.registration.event,
                        ExtensionDiagnosticCode::QueueOverflow,
                        format!("coalesced {dropped} queued observation update(s)"),
                    ));
                }
                if self.health.is_open() {
                    current = queue.next();
                    continue;
                }
                let result = self
                    .engine
                    .invoke_controlled_with_effects(
                        &self.key,
                        &job.envelope,
                        self.registration.registration_id,
                        self.config.deadline(self.registration.deadline),
                        job.cancellation,
                        self.config.broker_operation_quota,
                    )
                    .await
                    .map_err(|error| (diagnostic_code(error.kind), error.message))
                    .map(|result| {
                        self.registration_runtime.apply_disposals(
                            &EffectOwner {
                                extension_id: self.extension_id.clone(),
                                session_id: self.session_id.clone(),
                            },
                            &result.disposed_registration_ids,
                        );
                        result.value
                    })
                    .and_then(|value| {
                        super::dispatch_result::decode_batch(value)
                            .map_err(|error| (ExtensionDiagnosticCode::ContractViolation, error))
                    })
                    .and_then(|batch| self.validate(batch));
                match result {
                    Ok(()) => self.health.record_success(),
                    Err((code, message)) => {
                        self.diagnostics.lock().push(diagnostics::invocation(
                            self.extension_id.clone(),
                            self.session_id.clone(),
                            self.registration.event,
                            code,
                            message,
                        ));
                        if code != ExtensionDiagnosticCode::Cancelled
                            && self
                                .health
                                .record_failure(self.config.circuit_failure_threshold)
                        {
                            self.catalog.write().set_effective_status(
                                &self.extension_id,
                                ExtensionCatalogStatus::Disabled,
                                Some(ExtensionDiagnosticCode::CircuitOpened),
                            );
                            self.diagnostics.lock().push(diagnostics::circuit_opened(
                                self.extension_id.clone(),
                                self.session_id.clone(),
                            ));
                            self.registration_runtime.dispose_owner(&EffectOwner {
                                extension_id: self.extension_id.clone(),
                                session_id: self.session_id.clone(),
                            });
                            self.engine.dispose(&self.key).await;
                        }
                    }
                }
                current = queue.next();
            }
        });
    }

    fn validate(
        &self,
        batch: ExtensionActionBatch,
    ) -> Result<(), (ExtensionDiagnosticCode, String)> {
        if batch.actions.len() > self.config.max_actions {
            return Err((
                ExtensionDiagnosticCode::ResourceLimit,
                "extension action count exceeds the configured limit".into(),
            ));
        }
        self.registration
            .contract
            .validate_result(&batch)
            .map_err(|error| (ExtensionDiagnosticCode::ContractViolation, error.message))
    }
}

pub(super) fn diagnostic_code(kind: EngineInvocationErrorKind) -> ExtensionDiagnosticCode {
    match kind {
        EngineInvocationErrorKind::Timeout => ExtensionDiagnosticCode::HookTimedOut,
        EngineInvocationErrorKind::Cancelled => ExtensionDiagnosticCode::Cancelled,
        EngineInvocationErrorKind::ResourceLimit => ExtensionDiagnosticCode::ResourceLimit,
        EngineInvocationErrorKind::Runtime => ExtensionDiagnosticCode::HookFailed,
    }
}
