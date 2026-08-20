use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CatchResultExt, Context, Function, Module, Promise, Runtime};
use serde_json::Value;
use tokio::sync::oneshot;

use super::brokers;
use super::catalog::ExtensionPackage;

const LOAD_TIMEOUT: Duration = Duration::from_secs(2);

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

impl EngineInvocationError {
    fn new(kind: EngineInvocationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

const HOST_BOOTSTRAP_SOURCE: &str = r#"
globalThis.__thewayHandlers = Object.create(null);
globalThis.__thewayRegistrationSequence = 0;

globalThis.__thewaySetup = async function () {
  try {
    const candidate = globalThis.__thewayExtension.default;
    const setup = typeof candidate === "function" ? candidate : candidate?.setup;
    if (typeof setup !== "function") {
      throw new TypeError("extension default export must be created by defineExtension");
    }
    const api = Object.freeze({
      abiMajor: 2,
      capabilities: Object.freeze({ has: () => false }),
      on(event, descriptor, handler) {
        if (typeof descriptor === "function") {
          handler = descriptor;
          descriptor = {};
        }
        if (typeof event !== "string" || event.length === 0 || typeof handler !== "function") {
          throw new TypeError("api.on requires an event name and handler");
        }
        const registration = {
          id: globalThis.__thewayRegistrationSequence,
          event,
          descriptor: descriptor ?? {},
          handler,
          priority: Number.isFinite(descriptor?.priority) ? descriptor.priority : 0,
          sequence: globalThis.__thewayRegistrationSequence++,
          disposed: false,
        };
        (globalThis.__thewayHandlers[event] ??= []).push(registration);
        return Object.freeze({
          dispose() { registration.disposed = true; },
        });
      },
    });
    await setup(api);
    return JSON.stringify({
      ok: true,
      value: {
        registrations: Object.values(globalThis.__thewayHandlers)
          .flat()
          .map(({ id, event, descriptor, sequence }) => ({
            registrationId: id,
            event,
            descriptor,
            sequence,
          })),
      },
    });
  } catch (error) {
    return JSON.stringify({ error: String(error?.message ?? error) });
  }
};

globalThis.__thewayInvoke = async function (serializedEnvelope, registrationId) {
  try {
    const envelope = JSON.parse(serializedEnvelope);
    const registration = (globalThis.__thewayHandlers[envelope.event] ?? [])
      .find((candidate) => candidate.id === registrationId && !candidate.disposed);
    if (registration === undefined) {
      throw new Error("hook registration is unavailable");
    }
    const result = await registration.handler(envelope, envelope.context);
    return JSON.stringify({ ok: true, value: result ?? null });
  } catch (error) {
    return JSON.stringify({ error: String(error?.message ?? error) });
  }
};
"#;

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
}

enum EngineCommand {
    Load {
        key: EngineInstanceKey,
        source: String,
        response: oneshot::Sender<Result<Value, String>>,
    },
    Invoke {
        key: EngineInstanceKey,
        envelope: String,
        registration_id: u64,
        deadline: Instant,
        cancellation: Arc<AtomicBool>,
        broker_operation_limit: usize,
        response: oneshot::Sender<Result<Value, EngineInvocationError>>,
    },
    Dispose {
        key: EngineInstanceKey,
        response: oneshot::Sender<bool>,
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
        let worker_count = worker_count.max(1);
        let mut workers = Vec::with_capacity(worker_count);
        let mut joins = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (sender, receiver) = mpsc::channel();
            let worker_limits = limits;
            let join = std::thread::Builder::new()
                .name(format!("theway-js-{index}"))
                .spawn(move || run_worker(receiver, worker_limits))
                .expect("QuickJS engine worker must start");
            workers.push(sender);
            joins.push(join);
        }
        Self {
            inner: Arc::new(EnginePoolInner {
                workers,
                joins: parking_lot::Mutex::new(joins),
                limits,
            }),
        }
    }

    pub fn worker_count(&self) -> usize {
        self.inner.workers.len()
    }

    pub async fn load(
        &self,
        key: EngineInstanceKey,
        package: &ExtensionPackage,
    ) -> Result<Value, String> {
        let source = package.prepared_source()?;
        let (response, receiver) = oneshot::channel();
        self.send(
            &key,
            EngineCommand::Load {
                key: key.clone(),
                source,
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
            return false;
        }
        receiver.await.unwrap_or(false)
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

fn run_worker(receiver: mpsc::Receiver<EngineCommand>, limits: QuickJsEngineLimits) {
    let mut instances = HashMap::new();
    while let Ok(command) = receiver.recv() {
        match command {
            EngineCommand::Load {
                key,
                source,
                response,
            } => {
                let result = match instances.entry(key) {
                    std::collections::hash_map::Entry::Occupied(_) => {
                        Err("QuickJS extension instance is already loaded".into())
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        EngineInstance::new(entry.key(), &source, limits).map(
                            |(instance, subscriptions)| {
                                entry.insert(instance);
                                subscriptions
                            },
                        )
                    }
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
                let _ = response.send(instances.remove(&key).is_some());
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
    broker_quota: brokers::BrokerOperationQuota,
}

impl EngineInstance {
    fn new(
        key: &EngineInstanceKey,
        source: &str,
        limits: QuickJsEngineLimits,
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
            BuiltinResolver::default().with_module("theway"),
            BuiltinLoader::default().with_module("theway", host_module),
        );
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        interrupt.begin(
            Instant::now() + LOAD_TIMEOUT,
            Arc::new(AtomicBool::new(false)),
        );
        let evaluation = context.with(|ctx| {
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
            js(&ctx, ctx.eval::<(), _>(HOST_BOOTSTRAP_SOURCE))?;
            let setup: Function = js(&ctx, ctx.globals().get("__thewaySetup"))?;
            let promise: Promise = js(&ctx, setup.call(()))?;
            let output: String = js(&ctx, promise.finish())?;
            decode_host_result(&output, limits.serialized_output_bytes)
        });
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
                broker_quota: brokers::BrokerOperationQuota::new(),
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
    ) -> Result<Value, EngineInvocationError> {
        if cancellation.load(Ordering::Acquire) {
            return Err(EngineInvocationError::new(
                EngineInvocationErrorKind::Cancelled,
                "extension invocation was cancelled",
            ));
        }
        self.broker_quota.begin(broker_operation_limit);
        self.interrupt.begin(deadline, cancellation);
        let result = self.context.with(|ctx| {
            let invoke: Function = js(&ctx, ctx.globals().get("__thewayInvoke"))?;
            let promise: Promise = js(&ctx, invoke.call((envelope, registration_id)))?;
            let output: String = js(&ctx, promise.finish())?;
            decode_host_result(&output, self.output_limit)
        });
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
            {
                EngineInvocationErrorKind::ResourceLimit
            } else {
                EngineInvocationErrorKind::Runtime
            };
            EngineInvocationError::new(kind, message)
        })
    }
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
