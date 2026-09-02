//! Persistent QuickJS instance owned by one engine worker. Loading evaluates
//! the extension entry and runs setup; invocation executes one registration
//! under interrupt/cancellation budgets.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CatchResultExt, Context, Function, Module, Promise, Runtime};
use serde_json::Value;
use theway_contract::extension::ExtensionLifecycleEvent;

use super::broker_services::ExtensionBrokerServices;
use super::brokers;
use super::catalog::ExtensionPackage;
use super::engine::{
    DisposeReport, EngineInstanceKey, EngineInvocationError, EngineInvocationErrorKind,
    EngineInvocationResult, LOAD_TIMEOUT, QuickJsEngineLimits,
};

struct ActiveInterruptBudget {
    deadline: Instant,
    cancellation: Arc<AtomicBool>,
}

#[derive(Default)]
pub(super) struct InterruptBudget {
    active: parking_lot::Mutex<Option<ActiveInterruptBudget>>,
}

impl InterruptBudget {
    pub(super) fn begin(&self, deadline: Instant, cancellation: Arc<AtomicBool>) {
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

    pub(super) fn finish(&self) -> Option<EngineInvocationErrorKind> {
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

pub(super) struct EngineInstance {
    context: Context,
    _runtime: Runtime,
    pub(super) interrupt: Arc<InterruptBudget>,
    output_limit: usize,
    broker: Arc<brokers::BrokerRuntime>,
}

impl Drop for EngineInstance {
    fn drop(&mut self) {
        self.broker.clear_ephemeral_memory();
    }
}

impl EngineInstance {
    pub(super) fn new(
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

    pub(super) fn invoke(
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
    pub(super) fn run_disposers(&mut self) -> Result<DisposeReport, String> {
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
