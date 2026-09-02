use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CatchResultExt, Context, Function, Module, Promise, Runtime};
use serde_json::Value;
use theway_contract::extension::{
    ExtensionAction, ExtensionDurableEntry, ExtensionLifecycleEvent, ExtensionPermission,
};
use tokio::sync::oneshot;

use super::broker_services::ExtensionBrokerServices;
use super::brokers;
use super::catalog::{ExtensionPackage, PackageCatalog};

const LOAD_TIMEOUT: Duration = Duration::from_secs(2);
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
    fn new(kind: EngineInvocationErrorKind, message: impl Into<String>) -> Self {
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

struct ActiveInterruptBudget {
    deadline: Instant,
    cancellation: Arc<AtomicBool>,
}

#[derive(Default)]
struct InterruptBudget {
    active: parking_lot::Mutex<Option<ActiveInterruptBudget>>,
}

impl InterruptBudget {
    fn begin(&self, deadline: Instant, cancellation: Arc<AtomicBool>) {
        *self.active.lock() = Some(ActiveInterruptBudget {
            deadline,
            cancellation,
        });
    }

    fn should_interrupt(&self) -> bool {
        self.active.lock().as_ref().is_some_and(|budget| {
            budget.cancellation.load(Ordering::Acquire) || Instant::now() >= budget.deadline
        })
    }

    fn finish(&self) -> Option<EngineInvocationErrorKind> {
        self.active.lock().take().and_then(|budget| {
            if budget.cancellation.load(Ordering::Acquire) {
                Some(EngineInvocationErrorKind::Cancelled)
            } else if Instant::now() >= budget.deadline {
                Some(EngineInvocationErrorKind::Timeout)
            } else {
                None
            }
        })
    }
}

struct EngineInstance {
    context: Context,
    _runtime: Runtime,
    interrupt: Arc<InterruptBudget>,
    output_limit: usize,
    broker: Arc<brokers::BrokerRuntime>,
}

impl Drop for EngineInstance {
    fn drop(&mut self) {
        self.broker.clear_ephemeral_memory();
    }
}

impl EngineInstance {
    fn new(
        key: &EngineInstanceKey,
        source: &str,
        limits: QuickJsEngineLimits,
        package: &ExtensionPackage,
        broker_services: ExtensionBrokerServices,
        config: serde_json::Value,
    ) -> Result<(Self, Value), String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_memory_limit(limits.memory_bytes);
        runtime.set_max_stack_size(limits.stack_bytes);
        let interrupt = Arc::new(InterruptBudget::default());
        let interrupt_handler = Arc::clone(&interrupt);
        runtime.set_interrupt_handler(Some(Box::new(move || interrupt_handler.should_interrupt())));
        runtime
            .set_info(format!("theway:{}/{}", key.session_id, key.extension_id))
            .map_err(|error| error.to_string())?;
        let host_module = brokers::generated_theway_module();
        runtime.set_loader(
            BuiltinResolver::default().with_module(brokers::PLUGIN_SDK_MODULE),
            BuiltinLoader::default().with_module(brokers::PLUGIN_SDK_MODULE, host_module),
        );
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        let broker = Arc::new(brokers::BrokerRuntime::new(key, package, broker_services));
        broker.set_config(config);
        let load_cancellation = Arc::new(AtomicBool::new(false));
        let load_deadline = Instant::now() + LOAD_TIMEOUT;
        interrupt.begin(load_deadline, Arc::clone(&load_cancellation));
        broker.begin(
            32,
            load_cancellation,
            load_deadline,
            ExtensionLifecycleEvent::ExtensionLoad,
            Value::Null,
        );
        let evaluation = context.with(|ctx| {
            let broker_handler = Arc::clone(&broker);
            let broker_function = js(
                &ctx,
                Function::new(ctx.clone(), move |operation: String, arguments: String| {
                    broker_handler.call(&operation, &arguments)
                }),
            )?;
            js(&ctx, ctx.globals().set("__thewayBroker", broker_function))?;
            for name in brokers::FORBIDDEN_DIRECT_GLOBALS {
                if js(&ctx, ctx.globals().contains_key(*name))? {
                    return Err(format!(
                        "QuickJS context unexpectedly exposes forbidden global {name}"
                    ));
                }
            }
            let module = js(
                &ctx,
                Module::declare(ctx.clone(), "theway-extension", source),
            )?;
            let (module, promise) = js(&ctx, module.eval())?;
            js(&ctx, promise.finish::<()>())?;
            let namespace = js(&ctx, module.namespace())?;
            js(&ctx, ctx.globals().set("__thewayExtension", namespace))?;
            // Runtime identity (issue #86 v2 bridge surface).
            js(
                &ctx,
                ctx.globals()
                    .set("__thewayRuntimeVersion", env!("CARGO_PKG_VERSION")),
            )?;
            js(
                &ctx,
                ctx.globals()
                    .set("__thewayRuntimePluginId", key.extension_id.as_str()),
            )?;
            js(
                &ctx,
                ctx.globals()
                    .set("__thewayRuntimeSessionId", key.session_id.as_str()),
            )?;
            js(
                &ctx,
                ctx.eval::<(), _>(super::facade::HOST_BOOTSTRAP_SOURCE),
            )?;
            let setup: Function = js(&ctx, ctx.globals().get("__thewaySetup"))?;
            let promise: Promise = js(&ctx, setup.call(()))?;
            let output: String = js(&ctx, promise.finish())?;
            decode_host_result(&output, limits.serialized_output_bytes)
        });
        broker.finish();
        if let Some(reason) = interrupt.finish() {
            return Err(match reason {
                EngineInvocationErrorKind::Timeout => {
                    "extension setup exceeded its execution deadline".into()
                }
                EngineInvocationErrorKind::Cancelled => "extension setup was cancelled".into(),
                _ => "extension setup was interrupted".into(),
            });
        }
        let metadata = evaluation?;
        Ok((
            Self {
                context,
                _runtime: runtime,
                interrupt,
                output_limit: limits.serialized_output_bytes,
                broker,
            },
            metadata,
        ))
    }

    fn invoke(
        &mut self,
        envelope: &str,
        registration_id: u64,
        deadline: Instant,
        cancellation: Arc<AtomicBool>,
        broker_operation_limit: usize,
    ) -> Result<EngineInvocationResult, EngineInvocationError> {
        if cancellation.load(Ordering::Acquire) {
            return Err(EngineInvocationError::new(
                EngineInvocationErrorKind::Cancelled,
                "extension invocation was cancelled",
            ));
        }
        let decoded_envelope: Value = serde_json::from_str(envelope).map_err(|error| {
            EngineInvocationError::new(
                EngineInvocationErrorKind::Runtime,
                format!("extension envelope is invalid: {error}"),
            )
        })?;
        let event = serde_json::from_value(
            decoded_envelope
                .get("event")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(|error| {
            EngineInvocationError::new(
                EngineInvocationErrorKind::Runtime,
                format!("extension event is invalid: {error}"),
            )
        })?;
        let raw_payload = decoded_envelope
            .get("payload")
            .cloned()
            .unwrap_or(Value::Null);
        self.broker.begin(
            broker_operation_limit,
            Arc::clone(&cancellation),
            deadline,
            event,
            raw_payload,
        );
        self.interrupt.begin(deadline, cancellation);
        let result = self.context.with(|ctx| {
            let invoke: Function = js(&ctx, ctx.globals().get("__thewayInvoke"))?;
            let promise: Promise = js(&ctx, invoke.call((envelope, registration_id)))?;
            let output: String = js(&ctx, promise.finish())?;
            decode_invocation_result(&output, self.output_limit)
        });
        self.broker.finish();
        if let Some(reason) = self.interrupt.finish() {
            let message = match reason {
                EngineInvocationErrorKind::Timeout => {
                    "extension invocation exceeded its execution deadline"
                }
                EngineInvocationErrorKind::Cancelled => "extension invocation was cancelled",
                _ => "extension invocation was interrupted",
            };
            return Err(EngineInvocationError::new(reason, message));
        }
        result.map_err(|message| {
            let lower = message.to_ascii_lowercase();
            let kind = if message.contains("serialized output limit")
                || lower.contains("out of memory")
                || lower.contains("allocation failed")
                || message == "null"
            {
                EngineInvocationErrorKind::ResourceLimit
            } else {
                EngineInvocationErrorKind::Runtime
            };
            EngineInvocationError::new(kind, message)
        })
    }

    /// Run the plugin's JS disposer queue (`api.effect` registrations) in
    /// reverse registration order, isolating per-disposer failures. Invoked
    /// once right before the VM is dropped during dispose. The caller owns the
    /// interrupt budget (dispose deadline).
    fn run_disposers(&mut self) -> Result<DisposeReport, String> {
        let script = "JSON.stringify(globalThis.__thewayDispose())";
        self.context.with(|ctx| {
            let output: String = ctx.eval(script).map_err(|e| e.to_string())?;
            serde_json::from_str(&output)
                .map_err(|error| format!("disposer report is invalid: {error}"))
        })
    }
}

fn decode_invocation_result(
    output: &str,
    output_limit: usize,
) -> Result<EngineInvocationResult, String> {
    let value = decode_host_result(output, output_limit)?;
    let result = value
        .get("result")
        .cloned()
        .ok_or_else(|| "extension invocation result is missing result".to_string())?;
    let disposed_registration_ids = serde_json::from_value(
        value
            .get("disposedRegistrationIds")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("extension invocation effect state is invalid: {error}"))?;
    let queued_durable_actions = serde_json::from_value(
        value
            .get("queuedDurableActions")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("extension invocation durable actions are invalid: {error}"))?;
    Ok(EngineInvocationResult {
        value: result,
        disposed_registration_ids,
        queued_durable_actions,
    })
}

fn js<'js, T>(ctx: &rquickjs::Ctx<'js>, result: rquickjs::Result<T>) -> Result<T, String> {
    result.catch(ctx).map_err(|error| error.to_string())
}

fn decode_host_result(output: &str, output_limit: usize) -> Result<Value, String> {
    if output.len() > output_limit {
        return Err("extension result exceeds the serialized output limit".into());
    }
    let decoded: Value = serde_json::from_str(output)
        .map_err(|error| format!("extension host returned invalid JSON: {error}"))?;
    if let Some(error) = decoded.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    decoded
        .get("value")
        .cloned()
        .or_else(|| decoded.get("ok").is_some().then_some(Value::Null))
        .ok_or_else(|| "extension host returned an invalid result envelope".to_string())
}
