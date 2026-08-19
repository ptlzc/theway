use std::sync::Arc;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use theway_core::{
    ObservationContext, OperationDetail, OperationOutcome, OperationScope, RuntimeMeasurements,
    RuntimeObserver,
};

use super::*;

fn metrics() -> Arc<RuntimeMetrics> {
    Arc::new(RuntimeMetrics {
        prometheus: PrometheusMetrics::new(),
        otel: None,
    })
}

#[test]
fn bounded_queue_counts_drops_without_blocking() {
    let (tx, _rx) = mpsc::sync_channel(1);
    let runtime_metrics = metrics();
    let observer = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: Arc::new(AtomicBool::new(false)),
        dropped: AtomicU64::new(0),
        metrics: runtime_metrics,
    });
    let observer_port: Arc<dyn RuntimeObserver> = observer.clone();
    let first = OperationScope::start(
        observer_port.clone(),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    let second = OperationScope::start(
        observer_port,
        Some(first.id()),
        ObservationContext::default(),
        OperationDetail::Turn { index: 0 },
    );
    std::mem::forget(first);
    std::mem::forget(second);

    assert!(observer.dropped.load(Ordering::Relaxed) >= 1);
    let families = observer.metrics.prometheus.registry.gather();
    let dropped = families
        .iter()
        .find(|family| family.name() == "theway_runtime_observations_dropped_total")
        .expect("drop metric");
    assert!(dropped.get_metric()[0].get_counter().value() >= 1.0);
}

#[test]
fn disconnected_worker_is_isolated_and_counted_as_dropped() {
    let (tx, rx) = mpsc::sync_channel(1);
    drop(rx);
    let runtime_metrics = metrics();
    let observer = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: Arc::new(AtomicBool::new(false)),
        dropped: AtomicU64::new(0),
        metrics: runtime_metrics,
    });
    let observer_port: Arc<dyn RuntimeObserver> = observer.clone();

    OperationScope::start(
        observer_port,
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    )
    .finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements::default(),
    );

    assert_eq!(observer.dropped.load(Ordering::Relaxed), 2);
}

#[test]
fn official_sdk_spans_share_trace_and_parent_identity() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("theway-observability-test");
    let runtime_metrics = metrics();
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(16);
    let worker_stopped = stopped.clone();
    let worker_metrics = runtime_metrics.clone();
    let worker = std::thread::spawn(move || {
        worker_loop(rx, Some(tracer), worker_metrics, worker_stopped);
    });
    let observer: Arc<dyn RuntimeObserver> = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: stopped.clone(),
        dropped: AtomicU64::new(0),
        metrics: runtime_metrics,
    });

    let parent = OperationScope::start(
        observer.clone(),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    let child = OperationScope::start(
        observer,
        Some(parent.id()),
        ObservationContext::default().with_turn(1),
        OperationDetail::Turn { index: 1 },
    );
    child.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements::default(),
    );
    parent.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements::default(),
    );
    stopped.store(true, Ordering::Release);
    worker.join().unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    let parent = spans
        .iter()
        .find(|span| span.name == "agent.run")
        .expect("agent run span");
    let child = spans
        .iter()
        .find(|span| span.name == "agent.turn")
        .expect("turn span");
    assert_eq!(child.span_context.trace_id(), parent.span_context.trace_id());
    assert_eq!(child.parent_span_id, parent.span_context.span_id());
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn prometheus_endpoint_has_bounded_labels_and_exact_measurements() {
    let handle = TelemetryHandle::from_config(TelemetryConfig {
        otlp_enabled: false,
        metrics_addr: Some("127.0.0.1:0".parse().unwrap()),
        queue_capacity: 16,
    })
    .await;
    assert!(handle.tracer_provider.is_none());
    assert!(handle.meter_provider.is_none());
    let observer = handle.observer();
    let parent = OperationScope::start(
        observer.clone(),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    let scope = OperationScope::start(
        observer,
        Some(parent.id()),
        ObservationContext {
            session_id: Some("secret-session-id".into()),
            run_id: Some("secret-run-id".into()),
            ..ObservationContext::default()
        },
        OperationDetail::LlmRequest {
            provider: "provider-a".into(),
            model: "model-a".into(),
        },
    );
    scope.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements {
            input_tokens: 11,
            output_tokens: 7,
            ..RuntimeMeasurements::default()
        },
    );
    parent.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements {
            input_tokens: 11,
            output_tokens: 7,
            turns: 1,
            ..RuntimeMeasurements::default()
        },
    );
    tokio::time::sleep(Duration::from_millis(75)).await;

    let addr = handle.metrics_addr().expect("metrics listener");
    let text = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let completed = text
        .lines()
        .find(|line| {
            line.starts_with("theway_runtime_operations_total{")
                && line.contains("operation=\"llm.request\"")
        });
    assert!(completed.is_some_and(|line| {
        line.contains("operation=\"llm.request\"")
            && line.contains("outcome=\"succeeded\"")
            && line.contains("error_category=\"none\"")
            && line.ends_with(" 1")
    }));
    let input = text.lines().find(|line| {
        line.starts_with("theway_runtime_tokens_total{")
            && line.contains("direction=\"input\"")
    });
    let output = text.lines().find(|line| {
        line.starts_with("theway_runtime_tokens_total{")
            && line.contains("direction=\"output\"")
    });
    assert!(input.is_some_and(|line| line.ends_with(" 11")));
    assert!(output.is_some_and(|line| line.ends_with(" 7")));
    for secret in ["secret-session-id", "secret-run-id"] {
        assert!(!text.contains(secret));
    }
    handle.shutdown().await;
}
