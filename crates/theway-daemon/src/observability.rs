//! Daemon-owned export for the transport-neutral runtime observations emitted by core.
//!
//! The hot path performs one bounded `try_send`. A dedicated worker owns trace spans,
//! structured operational logs, and metric instruments. Export failures and queue pressure
//! never enter agent control flow.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::{Span as _, Status, TraceContextExt, Tracer as _, TracerProvider as _};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use theway_core::{
    ErrorCategory, OperationDetail, OperationFinished, OperationId, OperationKind,
    OperationOutcome, RuntimeMeasurements, RuntimeObservation, RuntimeObserver,
};

mod metrics_server;
mod runtime_metrics;

use metrics_server::MetricsServer;
use runtime_metrics::{OtelMetrics, PrometheusMetrics, RuntimeMetrics};

const DEFAULT_QUEUE_CAPACITY: usize = 4_096;
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(25);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub otlp_enabled: bool,
    pub metrics_addr: Option<SocketAddr>,
    pub queue_capacity: usize,
}

impl TelemetryConfig {
    pub fn from_env() -> Self {
        let otlp_enabled = [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        ]
        .iter()
        .any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|endpoint| !endpoint.trim().is_empty())
        });
        let metrics_addr = std::env::var("THEWAY_METRICS_ADDR")
            .ok()
            .and_then(|value| match value.trim().parse() {
                Ok(addr) => Some(addr),
                Err(error) => {
                    tracing::warn!(
                        target: "theway::observability",
                        %error,
                        "ignoring invalid THEWAY_METRICS_ADDR"
                    );
                    None
                }
            });
        let queue_capacity = std::env::var("THEWAY_OBSERVABILITY_QUEUE_CAPACITY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|capacity| *capacity > 0)
            .unwrap_or(DEFAULT_QUEUE_CAPACITY);
        Self {
            otlp_enabled,
            metrics_addr,
            queue_capacity,
        }
    }
}

pub struct TelemetryHandle {
    observer: Arc<DaemonRuntimeObserver>,
    worker: Option<JoinHandle<()>>,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    metrics_server: Option<MetricsServer>,
}

impl TelemetryHandle {
    pub async fn init() -> Self {
        Self::from_config(TelemetryConfig::from_env()).await
    }

    pub async fn from_config(config: TelemetryConfig) -> Self {
        let prometheus = PrometheusMetrics::new();
        let (tracer_provider, meter_provider, tracer, otel_metrics) = if config.otlp_enabled {
            match build_otel() {
                Ok(parts) => parts,
                Err(error) => {
                    tracing::warn!(
                        target: "theway::observability",
                        %error,
                        "OpenTelemetry export is disabled"
                    );
                    (None, None, None, None)
                }
            }
        } else {
            (None, None, None, None)
        };
        let metrics = Arc::new(RuntimeMetrics {
            prometheus,
            otel: otel_metrics,
        });
        let (tx, rx) = mpsc::sync_channel(config.queue_capacity);
        let observer = Arc::new(DaemonRuntimeObserver {
            tx,
            stopped: Arc::new(AtomicBool::new(false)),
            dropped: AtomicU64::new(0),
            metrics: Arc::clone(&metrics),
        });
        let stopped = Arc::clone(&observer.stopped);
        let worker = std::thread::Builder::new()
            .name("theway-observability".into())
            .spawn(move || worker_loop(rx, tracer, metrics, stopped))
            .ok();
        let metrics_server = match config.metrics_addr {
            Some(addr) => MetricsServer::spawn(addr, observer.metrics.prometheus.registry()).await,
            None => None,
        };
        Self {
            observer,
            worker,
            tracer_provider,
            meter_provider,
            metrics_server,
        }
    }

    pub fn observer(&self) -> Arc<dyn RuntimeObserver> {
        self.observer.clone()
    }

    #[cfg(test)]
    pub fn metrics_addr(&self) -> Option<SocketAddr> {
        self.metrics_server.as_ref().map(|server| server.addr)
    }

    pub async fn shutdown(mut self) {
        self.observer.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if tokio::time::timeout(
                SHUTDOWN_TIMEOUT,
                tokio::task::spawn_blocking(move || worker.join()),
            )
            .await
            .is_err()
            {
                tracing::warn!(target: "theway::observability", "observation drain timed out");
            }
        }
        if let Some(server) = self.metrics_server.take() {
            server.shutdown().await;
        }
        if let Some(provider) = self.tracer_provider.take() {
            match tokio::time::timeout(
                SHUTDOWN_TIMEOUT,
                tokio::task::spawn_blocking(move || provider.shutdown()),
            )
            .await
            {
                Ok(Ok(Err(error))) => {
                    tracing::warn!(target: "theway::observability", %error, "trace flush failed");
                }
                Err(_) => {
                    tracing::warn!(target: "theway::observability", "trace flush timed out");
                }
                Ok(Err(error)) => {
                    tracing::warn!(target: "theway::observability", %error, "trace flush task failed");
                }
                Ok(Ok(Ok(()))) => {}
            }
        }
        if let Some(provider) = self.meter_provider.take() {
            match tokio::time::timeout(
                SHUTDOWN_TIMEOUT,
                tokio::task::spawn_blocking(move || provider.shutdown()),
            )
            .await
            {
                Ok(Ok(Err(error))) => {
                    tracing::warn!(target: "theway::observability", %error, "metric flush failed");
                }
                Err(_) => {
                    tracing::warn!(target: "theway::observability", "metric flush timed out");
                }
                Ok(Err(error)) => {
                    tracing::warn!(target: "theway::observability", %error, "metric flush task failed");
                }
                Ok(Ok(Ok(()))) => {}
            }
        }
    }
}

pub struct DaemonRuntimeObserver {
    tx: mpsc::SyncSender<RuntimeObservation>,
    stopped: Arc<AtomicBool>,
    dropped: AtomicU64,
    metrics: Arc<RuntimeMetrics>,
}

impl RuntimeObserver for DaemonRuntimeObserver {
    fn observe(&self, observation: RuntimeObservation) {
        if self.stopped.load(Ordering::Acquire) {
            self.record_drop();
            return;
        }
        if self.tx.try_send(observation).is_err() {
            self.record_drop();
        }
    }
}

impl DaemonRuntimeObserver {
    fn record_drop(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        self.metrics.record_drop();
    }
}

fn build_otel() -> anyhow::Result<(
    Option<SdkTracerProvider>,
    Option<SdkMeterProvider>,
    Option<SdkTracer>,
    Option<OtelMetrics>,
)> {
    let resource = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", "thewayd"),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("service.instance.id", uuid::Uuid::new_v4().to_string()),
        ])
        .build();
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .with_resource(resource.clone())
        .build();
    let tracer = tracer_provider.tracer("theway-daemon");

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .build()?;
    let reader = PeriodicReader::builder(metric_exporter).build();
    let meter_provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();
    let meter = meter_provider.meter("theway-daemon");
    let metrics = OtelMetrics::new(&meter);
    Ok((
        Some(tracer_provider),
        Some(meter_provider),
        Some(tracer),
        Some(metrics),
    ))
}

struct ActiveOperation {
    trace_context: Option<Context>,
    kind: OperationKind,
    metric_context: MetricContext,
}

#[derive(Default)]
struct MetricContext {
    provider: Option<String>,
    model: Option<String>,
}

impl MetricContext {
    fn from_detail(detail: &OperationDetail) -> Self {
        match detail {
            OperationDetail::LlmRequest { provider, model }
            | OperationDetail::Compaction {
                provider, model, ..
            } => Self {
                provider: Some(provider.clone()),
                model: Some(model.clone()),
            },
            _ => Self::default(),
        }
    }
}

fn worker_loop(
    rx: mpsc::Receiver<RuntimeObservation>,
    tracer: Option<SdkTracer>,
    metrics: Arc<RuntimeMetrics>,
    stopped: Arc<AtomicBool>,
) {
    let mut active = HashMap::<OperationId, ActiveOperation>::new();
    loop {
        match rx.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(RuntimeObservation::OperationStarted(start)) => {
                let kind = start.detail.kind();
                let metric_context = MetricContext::from_detail(&start.detail);
                metrics.record_start(kind);
                log_start(&start);
                let trace_context = tracer.as_ref().map(|tracer| {
                    let parent = start
                        .parent_id
                        .and_then(|id| active.get(&id))
                        .and_then(|operation| operation.trace_context.clone())
                        .unwrap_or_else(Context::new);
                    let mut span = tracer.start_with_context(kind.as_str(), &parent);
                    for attribute in trace_attributes(&start) {
                        span.set_attribute(attribute);
                    }
                    Context::new().with_span(span)
                });
                active.insert(
                    start.id,
                    ActiveOperation {
                        trace_context,
                        kind,
                        metric_context,
                    },
                );
            }
            Ok(RuntimeObservation::OperationFinished(finish)) => {
                let started = active.remove(&finish.id);
                metrics.record_finish(&finish, started.as_ref());
                log_finish(&finish);
                if let Some(operation) = started {
                    if let Some(context) = operation.trace_context {
                        finish_span(&context, &finish);
                    }
                }
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if stopped.load(Ordering::Acquire) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    for (_, operation) in active {
        metrics.record_abandoned(operation.kind);
        if let Some(context) = operation.trace_context {
            let span = context.span();
            span.set_status(Status::error(OperationOutcome::Abandoned.as_str()));
            span.end();
        }
    }
}

fn trace_attributes(start: &theway_core::OperationStarted) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new("theway.operation.id", start.id.get() as i64),
        KeyValue::new("theway.operation.kind", start.detail.kind().as_str()),
    ];
    if let Some(parent) = start.parent_id {
        attributes.push(KeyValue::new(
            "theway.operation.parent_id",
            parent.get() as i64,
        ));
    }
    push_context_attributes(&mut attributes, &start.context);
    match &start.detail {
        OperationDetail::Turn { index } => {
            attributes.push(KeyValue::new("theway.turn.index", i64::from(*index)));
        }
        OperationDetail::LlmRequest { provider, model } => {
            attributes.push(KeyValue::new("gen_ai.provider.name", provider.clone()));
            attributes.push(KeyValue::new("gen_ai.request.model", model.clone()));
        }
        OperationDetail::ToolExecution { tool_name } => {
            attributes.push(KeyValue::new("theway.tool.name", tool_name.clone()));
        }
        OperationDetail::Compaction {
            algorithm,
            provider,
            model,
        } => {
            attributes.push(KeyValue::new(
                "theway.compaction.algorithm",
                algorithm.clone(),
            ));
            attributes.push(KeyValue::new("gen_ai.provider.name", provider.clone()));
            attributes.push(KeyValue::new("gen_ai.request.model", model.clone()));
        }
        OperationDetail::SubagentJob { agent, source } => {
            attributes.push(KeyValue::new("theway.agent.name", agent.clone()));
            attributes.push(KeyValue::new("theway.agent.source", source.clone()));
        }
        OperationDetail::AgentRun | OperationDetail::DagRun | OperationDetail::DagNode => {}
        _ => {}
    }
    attributes
}

fn push_context_attributes(
    attributes: &mut Vec<KeyValue>,
    context: &theway_core::ObservationContext,
) {
    for (key, value) in [
        ("theway.session.id", context.session_id.as_deref()),
        ("theway.run.id", context.run_id.as_deref()),
        ("theway.job.id", context.job_id.as_deref()),
        ("theway.node.id", context.node_id.as_deref()),
    ] {
        if let Some(value) = value {
            attributes.push(KeyValue::new(key, value.to_string()));
        }
    }
    if let Some(turn) = context.turn_id {
        attributes.push(KeyValue::new("theway.turn.id", i64::from(turn)));
    }
}

fn finish_span(context: &Context, finish: &OperationFinished) {
    let span = context.span();
    span.set_attribute(KeyValue::new("theway.outcome", finish.outcome.as_str()));
    span.set_attribute(KeyValue::new(
        "theway.duration_ms",
        finish.duration.as_secs_f64() * 1_000.0,
    ));
    if let Some(category) = finish.error_category {
        span.set_attribute(KeyValue::new("error.type", category.as_str()));
    }
    add_measurement_attributes(&span, finish.measurements);
    if matches!(
        finish.outcome,
        OperationOutcome::Failed | OperationOutcome::TimedOut | OperationOutcome::Abandoned
    ) {
        span.set_status(Status::error(finish.outcome.as_str()));
    } else {
        span.set_status(Status::Ok);
    }
    span.end();
}

fn add_measurement_attributes(
    span: &opentelemetry::trace::SpanRef<'_>,
    value: RuntimeMeasurements,
) {
    for (key, measurement) in [
        ("gen_ai.usage.input_tokens", value.input_tokens),
        ("gen_ai.usage.output_tokens", value.output_tokens),
        ("theway.cache.read_tokens", value.cache_read_tokens),
        ("theway.cache.write_tokens", value.cache_write_tokens),
        ("theway.characters", value.characters),
        ("theway.turns", value.turns),
        ("theway.tool_calls", value.tool_calls),
    ] {
        if measurement > 0 {
            span.set_attribute(KeyValue::new(key, measurement as i64));
        }
    }
}

fn log_start(start: &theway_core::OperationStarted) {
    tracing::info!(
        target: "theway::runtime",
        event = "operation_started",
        operation = start.detail.kind().as_str(),
        operation_id = start.id.get(),
        parent_operation_id = start.parent_id.map(OperationId::get),
        session_id = start.context.session_id.as_deref(),
        run_id = start.context.run_id.as_deref(),
        job_id = start.context.job_id.as_deref(),
        node_id = start.context.node_id.as_deref(),
    );
}

fn log_finish(finish: &OperationFinished) {
    tracing::info!(
        target: "theway::runtime",
        event = "operation_finished",
        operation = finish.kind.as_str(),
        operation_id = finish.id.get(),
        outcome = finish.outcome.as_str(),
        error_category = finish.error_category.map(ErrorCategory::as_str),
        duration_ms = finish.duration.as_secs_f64() * 1_000.0,
        input_tokens = finish.measurements.input_tokens,
        output_tokens = finish.measurements.output_tokens,
        cache_read_tokens = finish.measurements.cache_read_tokens,
        cache_write_tokens = finish.measurements.cache_write_tokens,
        characters = finish.measurements.characters,
        turns = finish.measurements.turns,
        tool_calls = finish.measurements.tool_calls,
    );
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("observability");
