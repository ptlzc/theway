use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, mpsc};

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CatchResultExt, Context, Function, Module, Promise, Runtime};
use serde_json::Value;
use theway_contract::extension::ExtensionLifecycleEvent;
use tokio::sync::oneshot;

use super::brokers;
use super::catalog::ExtensionPackage;

const DEFAULT_MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const DEFAULT_STACK_LIMIT: usize = 512 * 1024;

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
      value: { events: Object.keys(globalThis.__thewayHandlers) },
    });
  } catch (error) {
    return JSON.stringify({ error: String(error?.message ?? error) });
  }
};

globalThis.__thewayInvoke = async function (serializedEnvelope) {
  try {
    const envelope = JSON.parse(serializedEnvelope);
    const handlers = [...(globalThis.__thewayHandlers[envelope.event] ?? [])]
      .filter((registration) => !registration.disposed)
      .sort((left, right) => right.priority - left.priority || left.sequence - right.sequence);
    const aggregate = { abiMajor: 2, actions: [] };
    for (const registration of handlers) {
      const result = await registration.handler(envelope, envelope.context);
      if (result == null) continue;
      if (result.decision != null) aggregate.decision = result.decision;
      if (Array.isArray(result.actions)) aggregate.actions.push(...result.actions);
    }
    return JSON.stringify({ ok: true, value: aggregate });
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
}

enum EngineCommand {
    Load {
        key: EngineInstanceKey,
        source: String,
        response: oneshot::Sender<Result<BTreeSet<ExtensionLifecycleEvent>, String>>,
    },
    Invoke {
        key: EngineInstanceKey,
        envelope: String,
        response: oneshot::Sender<Result<Value, String>>,
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
        let worker_count = worker_count.max(1);
        let mut workers = Vec::with_capacity(worker_count);
        let mut joins = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (sender, receiver) = mpsc::channel();
            let join = std::thread::Builder::new()
                .name(format!("theway-js-{index}"))
                .spawn(move || run_worker(receiver))
                .expect("QuickJS engine worker must start");
            workers.push(sender);
            joins.push(join);
        }
        Self {
            inner: Arc::new(EnginePoolInner {
                workers,
                joins: parking_lot::Mutex::new(joins),
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
    ) -> Result<BTreeSet<ExtensionLifecycleEvent>, String> {
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
    ) -> Result<Value, String> {
        let envelope = serde_json::to_string(envelope)
            .map_err(|error| format!("extension envelope serialization failed: {error}"))?;
        let (response, receiver) = oneshot::channel();
        self.send(
            key,
            EngineCommand::Invoke {
                key: key.clone(),
                envelope,
                response,
            },
        )?;
        receiver
            .await
            .map_err(|_| "QuickJS engine worker stopped during invocation".to_string())?
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

fn run_worker(receiver: mpsc::Receiver<EngineCommand>) {
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
                        EngineInstance::new(entry.key(), &source).map(
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
                response,
            } => {
                let result = instances
                    .get_mut(&key)
                    .ok_or_else(|| "QuickJS extension instance is not loaded".to_string())
                    .and_then(|instance| instance.invoke(&envelope));
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

struct EngineInstance {
    context: Context,
    _runtime: Runtime,
}

impl EngineInstance {
    fn new(
        key: &EngineInstanceKey,
        source: &str,
    ) -> Result<(Self, BTreeSet<ExtensionLifecycleEvent>), String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_memory_limit(DEFAULT_MEMORY_LIMIT);
        runtime.set_max_stack_size(DEFAULT_STACK_LIMIT);
        runtime
            .set_info(format!("theway:{}/{}", key.session_id, key.extension_id))
            .map_err(|error| error.to_string())?;
        let host_module = brokers::generated_theway_module();
        runtime.set_loader(
            BuiltinResolver::default().with_module("theway"),
            BuiltinLoader::default().with_module("theway", host_module),
        );
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        context
            .with(|ctx| {
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
                let value = decode_host_result(&output)?;
                let events = value
                    .get("events")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        "extension setup did not return its subscription index".to_string()
                    })?;
                events
                    .iter()
                    .map(|event| {
                        serde_json::from_value(event.clone()).map_err(|error| {
                            format!("extension registered an invalid event: {error}")
                        })
                    })
                    .collect::<Result<BTreeSet<_>, _>>()
            })
            .map(|subscriptions| {
                (
                    Self {
                        context,
                        _runtime: runtime,
                    },
                    subscriptions,
                )
            })
    }

    fn invoke(&mut self, envelope: &str) -> Result<Value, String> {
        self.context.with(|ctx| {
            let invoke: Function = js(&ctx, ctx.globals().get("__thewayInvoke"))?;
            let promise: Promise = js(&ctx, invoke.call((envelope,)))?;
            let output: String = js(&ctx, promise.finish())?;
            decode_host_result(&output)
        })
    }
}

fn js<'js, T>(ctx: &rquickjs::Ctx<'js>, result: rquickjs::Result<T>) -> Result<T, String> {
    result.catch(ctx).map_err(|error| error.to_string())
}

fn decode_host_result(output: &str) -> Result<Value, String> {
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
