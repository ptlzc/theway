use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;
use theway_contract::extension::{ExtensionAction, ExtensionDurableEntry, ExtensionPermission};
use tokio::sync::oneshot;

use super::broker_services::ExtensionBrokerServices;
use super::catalog::{ExtensionPackage, PackageCatalog};
use super::engine_instance::EngineInstance;
use super::live_event::LiveEvent;

pub(super) const LOAD_TIMEOUT: Duration = Duration::from_secs(2);
/// Budget for running the JS disposer queue during instance dispose.
const DISPOSER_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug)]
pub struct QuickJsEngineLimits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub serialized_output_bytes: usize,
}

impl Default for QuickJsEngineLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 32 * 1024 * 1024,
            stack_bytes: 512 * 1024,
            serialized_output_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineInvocationErrorKind {
    Timeout,
    Cancelled,
    ResourceLimit,
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineInvocationError {
    pub kind: EngineInvocationErrorKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EngineInvocationResult {
    pub value: Value,
    pub disposed_registration_ids: Vec<u64>,
    pub queued_durable_actions: Vec<ExtensionAction>,
}

/// Result of running the plugin's JS disposer queue before VM teardown.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisposeReport {
    /// How many registered disposers ran.
    pub executed: usize,
    /// Per-disposer failure messages (one throwing disposer does not stop the rest).
    pub errors: Vec<String>,
}

impl EngineInvocationError {
    pub(super) fn new(kind: EngineInvocationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EngineInstanceKey {
    pub session_id: String,
    pub extension_id: String,
}

impl EngineInstanceKey {
    pub fn new(session_id: impl Into<String>, extension_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            extension_id: extension_id.into(),
        }
    }
}

#[derive(Clone)]
pub struct QuickJsEnginePool {
    inner: Arc<EnginePoolInner>,
}

struct EnginePoolInner {
    workers: Vec<mpsc::Sender<EngineCommand>>,
    joins: parking_lot::Mutex<Vec<std::thread::JoinHandle<()>>>,
    limits: QuickJsEngineLimits,
    broker_services: ExtensionBrokerServices,
}

enum EngineCommand {
    Load {
        key: EngineInstanceKey,
        source: String,
        package: ExtensionPackage,
        config: serde_json::Value,
        response: oneshot::Sender<Result<Value, String>>,
    },
    Invoke {
        key: EngineInstanceKey,
        envelope: String,
        registration_id: u64,
        deadline: Instant,
        cancellation: Arc<AtomicBool>,
        broker_operation_limit: usize,
        response: oneshot::Sender<Result<EngineInvocationResult, EngineInvocationError>>,
    },
    Dispose {
        key: EngineInstanceKey,
        response: oneshot::Sender<Option<DisposeReport>>,
    },
    Count {
        response: oneshot::Sender<usize>,
    },
    Shutdown,
}

impl Default for QuickJsEnginePool {
    fn default() -> Self {
        let workers = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(4);
        Self::new(workers)
    }
}

impl QuickJsEnginePool {
    pub fn new(worker_count: usize) -> Self {
        Self::with_limits(worker_count, QuickJsEngineLimits::default())
    }

    pub fn with_limits(worker_count: usize, limits: QuickJsEngineLimits) -> Self {
        Self::with_broker_services(worker_count, limits, ExtensionBrokerServices::default())
    }

    pub fn with_broker_services(
        worker_count: usize,
        limits: QuickJsEngineLimits,
        broker_services: ExtensionBrokerServices,
    ) -> Self {
        let worker_count = worker_count.max(1);
        let mut workers = Vec::with_capacity(worker_count);
        let mut joins = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (sender, receiver) = mpsc::channel();
            let worker_limits = limits;
            let worker_brokers = broker_services.clone();
            let join = std::thread::Builder::new()
                .name(format!("theway-js-{index}"))
                .spawn(move || run_worker(receiver, worker_limits, worker_brokers))
                .expect("QuickJS engine worker must start");
            workers.push(sender);
            joins.push(join);
        }
        Self {
            inner: Arc::new(EnginePoolInner {
                workers,
                joins: parking_lot::Mutex::new(joins),
                limits,
                broker_services,
            }),
        }
    }

    pub fn worker_count(&self) -> usize {
        self.inner.workers.len()
    }

    pub(super) fn isolated_pool(&self) -> Self {
        Self::with_broker_services(
            self.worker_count(),
            self.inner.limits,
            self.inner.broker_services.clone(),
        )
    }

    pub(crate) fn install_catalog_secrets(&self, catalog: &PackageCatalog) {
        for package in catalog.effective_packages() {
            for permission in package.granted_permissions() {
                if let ExtensionPermission::SecretsRead(name) = permission
                    && let Ok(value) = std::env::var(name)
                {
                    self.inner.broker_services.set_secret(name, value);
                }
            }
        }
    }

    pub async fn load(
        &self,
        key: EngineInstanceKey,
        package: &ExtensionPackage,
    ) -> Result<Value, String> {
        self.load_with_config(key, package, serde_json::Value::Null)
            .await
    }

    /// Load with a session config: the value is validated/merged by the host
    /// before this call and installed on the instance's broker before setup
    /// runs, so `api.getConfig()` inside `apply` returns the merged config.
    pub async fn load_with_config(
        &self,
        key: EngineInstanceKey,
        package: &ExtensionPackage,
        config: serde_json::Value,
    ) -> Result<Value, String> {
        let source = package.prepared_source()?;
        let (response, receiver) = oneshot::channel();
        self.send(
            &key,
            EngineCommand::Load {
                key: key.clone(),
                source,
                package: package.clone(),
                config,
                response,
            },
        )?;
        receiver
            .await
            .map_err(|_| "QuickJS engine worker stopped during load".to_string())?
    }

    pub async fn invoke<T: serde::Serialize>(
        &self,
        key: &EngineInstanceKey,
        envelope: &T,
        registration_id: u64,
    ) -> Result<Value, EngineInvocationError> {
        self.invoke_controlled(
            key,
            envelope,
            registration_id,
            Duration::from_millis(500),
            Arc::new(AtomicBool::new(false)),
            32,
        )
        .await
    }

    pub async fn invoke_controlled<T: serde::Serialize>(
        &self,
        key: &EngineInstanceKey,
        envelope: &T,
        registration_id: u64,
        timeout: Duration,
        cancellation: Arc<AtomicBool>,
        broker_operation_limit: usize,
    ) -> Result<Value, EngineInvocationError> {
        self.invoke_controlled_with_effects(
            key,
            envelope,
            registration_id,
            timeout,
            cancellation,
            broker_operation_limit,
        )
        .await
        .map(|result| result.value)
    }

    pub(super) async fn invoke_controlled_with_effects<T: serde::Serialize>(
        &self,
        key: &EngineInstanceKey,
        envelope: &T,
        registration_id: u64,
        timeout: Duration,
        cancellation: Arc<AtomicBool>,
        broker_operation_limit: usize,
    ) -> Result<EngineInvocationResult, EngineInvocationError> {
        let envelope = serde_json::to_string(envelope).map_err(|error| {
            EngineInvocationError::new(
                EngineInvocationErrorKind::Runtime,
                format!("extension envelope serialization failed: {error}"),
            )
        })?;
        if envelope.len() > self.inner.limits.serialized_output_bytes {
            return Err(EngineInvocationError::new(
                EngineInvocationErrorKind::ResourceLimit,
                "extension envelope exceeds the serialized input limit",
            ));
        }
        let (response, receiver) = oneshot::channel();
        self.send(
            key,
            EngineCommand::Invoke {
                key: key.clone(),
                envelope,
                registration_id,
                deadline: Instant::now() + timeout,
                cancellation,
                broker_operation_limit,
                response,
            },
        )
        .map_err(|message| {
            EngineInvocationError::new(EngineInvocationErrorKind::Runtime, message)
        })?;
        receiver.await.map_err(|_| {
            EngineInvocationError::new(
                EngineInvocationErrorKind::Runtime,
                "QuickJS engine worker stopped during invocation",
            )
        })?
    }

    pub async fn dispose(&self, key: &EngineInstanceKey) -> bool {
        self.dispose_with_report(key).await.is_some()
    }

    /// Dispose one instance, running its JS disposer queue first. Returns the
    /// disposer report when the instance existed; `None` when it did not.
    pub async fn dispose_with_report(&self, key: &EngineInstanceKey) -> Option<DisposeReport> {
        let (response, receiver) = oneshot::channel();
        if self
            .send(
                key,
                EngineCommand::Dispose {
                    key: key.clone(),
                    response,
                },
            )
            .is_err()
        {
            return None;
        }
        receiver.await.ok().flatten()
    }

    pub async fn instance_count(&self) -> usize {
        let mut total = 0;
        for worker in &self.inner.workers {
            let (response, receiver) = oneshot::channel();
            if worker.send(EngineCommand::Count { response }).is_ok() {
                total += receiver.await.unwrap_or(0);
            }
        }
        total
    }

    pub fn broker_diagnostics(
        &self,
        session_id: &str,
    ) -> Vec<theway_contract::extension::ExtensionDiagnostic> {
        self.inner.broker_services.diagnostics_for(session_id)
    }

    pub fn audit_log(&self) -> super::audit::ExtensionAuditLog {
        self.inner.broker_services.audit_log()
    }

    pub(crate) fn has_secret(&self, name: &str) -> bool {
        self.inner.broker_services.has_secret(name)
    }

    /// Register the live-event receiver for one session host. Broker calls to
    /// `events.publish` route through this session-keyed sender.
    pub(super) fn register_live_event_sender(
        &self,
        session_id: &str,
        sender: tokio::sync::mpsc::UnboundedSender<LiveEvent>,
    ) {
        self.inner
            .broker_services
            .live_events
            .register(session_id, sender);
    }

    pub(super) fn unregister_live_event_sender(&self, session_id: &str) {
        self.inner
            .broker_services
            .live_events
            .unregister(session_id);
    }

    pub(super) fn secret(&self, name: &str) -> Option<String> {
        self.inner.broker_services.secret(name)
    }

    /// Read a session-keyed plugin service value (service dependency gate).
    pub(super) fn service(&self, session_id: &str, name: &str) -> Option<serde_json::Value> {
        self.inner.broker_services.services.get(session_id, name)
    }

    /// Unregister every service a plugin provided in this session.
    pub(super) fn dispose_services(&self, session_id: &str, extension_id: &str) {
        self.inner
            .broker_services
            .services
            .dispose_owner(session_id, extension_id);
    }

    pub(super) fn install_extension_state(
        &self,
        key: &EngineInstanceKey,
        schema_version: Option<u32>,
        entries: &[ExtensionDurableEntry],
    ) {
        self.inner
            .broker_services
            .install_state(key, schema_version, entries);
    }

    pub(super) fn apply_extension_state(
        &self,
        key: &EngineInstanceKey,
        entries: &[ExtensionDurableEntry],
    ) {
        self.inner.broker_services.apply_state(key, entries);
    }

    fn send(&self, key: &EngineInstanceKey, command: EngineCommand) -> Result<(), String> {
        let worker = stable_worker(key, self.inner.workers.len());
        self.inner.workers[worker]
            .send(command)
            .map_err(|_| "QuickJS engine worker is unavailable".to_string())
    }
}

impl Drop for EnginePoolInner {
    fn drop(&mut self) {
        for worker in &self.workers {
            let _ = worker.send(EngineCommand::Shutdown);
        }
        for join in self.joins.get_mut().drain(..) {
            let _ = join.join();
        }
    }
}

fn stable_worker(key: &EngineInstanceKey, worker_count: usize) -> usize {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in key
        .session_id
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(key.extension_id.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as usize % worker_count
}

fn run_worker(
    receiver: mpsc::Receiver<EngineCommand>,
    limits: QuickJsEngineLimits,
    broker_services: ExtensionBrokerServices,
) {
    let mut instances = HashMap::new();
    while let Ok(command) = receiver.recv() {
        match command {
            EngineCommand::Load {
                key,
                source,
                package,
                config,
                response,
            } => {
                let result = match instances.entry(key) {
                    std::collections::hash_map::Entry::Occupied(_) => {
                        Err("QuickJS extension instance is already loaded".into())
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => EngineInstance::new(
                        entry.key(),
                        &source,
                        limits,
                        &package,
                        broker_services.clone(),
                        config,
                    )
                    .map(|(instance, subscriptions)| {
                        entry.insert(instance);
                        subscriptions
                    }),
                };
                let _ = response.send(result);
            }
            EngineCommand::Invoke {
                key,
                envelope,
                registration_id,
                deadline,
                cancellation,
                broker_operation_limit,
                response,
            } => {
                let result = instances
                    .get_mut(&key)
                    .ok_or_else(|| {
                        EngineInvocationError::new(
                            EngineInvocationErrorKind::Runtime,
                            "QuickJS extension instance is not loaded",
                        )
                    })
                    .and_then(|instance| {
                        instance.invoke(
                            &envelope,
                            registration_id,
                            deadline,
                            cancellation,
                            broker_operation_limit,
                        )
                    });
                let _ = response.send(result);
            }
            EngineCommand::Dispose { key, response } => {
                let report = match instances.remove(&key) {
                    Some(mut instance) => {
                        let deadline = Instant::now() + DISPOSER_TIMEOUT;
                        instance
                            .interrupt
                            .begin(deadline, Arc::new(AtomicBool::new(false)));
                        let report =
                            instance
                                .run_disposers()
                                .unwrap_or_else(|error| DisposeReport {
                                    executed: 0,
                                    errors: vec![error],
                                });
                        instance.interrupt.finish();
                        Some(report)
                    }
                    None => None,
                };
                let _ = response.send(report);
            }
            EngineCommand::Count { response } => {
                let _ = response.send(instances.len());
            }
            EngineCommand::Shutdown => break,
        }
    }
}
