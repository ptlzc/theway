use std::sync::Arc;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use theway_core::{
    ObservationContent, ObservationContext, OperationDetail, OperationOutcome, OperationScope,
    RuntimeMeasurements, RuntimeObserver,
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
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
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
    assert!(observer.status.snapshot().degraded);
    assert!(observer.status.snapshot().message.contains("queue full"));
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
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
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
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
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
    assert_eq!(
        child.span_context.trace_id(),
        parent.span_context.trace_id()
    );
    assert_eq!(child.parent_span_id, parent.span_context.span_id());
    provider.shutdown().unwrap();
}

#[test]
fn build_otel_constructs_http_exporters_with_configured_endpoint() {
    let config = TelemetryConfig {
        otlp_enabled: true,
        otlp_traces_endpoint: Some("http://127.0.0.1:4318".into()),
        otlp_traces_headers: Some(
            [("Authorization".into(), "Basic dGVzdA==".into())]
                .into_iter()
                .collect(),
        ),
        ..TelemetryConfig::default()
    };
    let (tracer_provider, meter_provider, tracer, metrics) =
        build_otel(&config, Arc::new(ObservabilityStatus::default()))
            .expect("OTLP HTTP client must be compiled in");
    assert!(tracer_provider.is_some());
    assert!(tracer.is_some());
    assert!(meter_provider.is_none());
    assert!(metrics.is_none());
    if let Some(provider) = tracer_provider {
        provider.shutdown().unwrap();
    }
}

#[test]
fn report_export_result_records_and_clears_status() {
    let status = Arc::new(ObservabilityStatus::default());

    report_export_result(
        &status,
        "OTLP trace export failed",
        &Err(opentelemetry_sdk::error::OTelSdkError::InternalFailure(
            "boom".into(),
        )),
    );
    let snapshot = status.snapshot();
    assert!(snapshot.degraded);
    assert!(snapshot.message.contains("OTLP trace export failed"));
    assert!(snapshot.message.contains("boom"), "{}", snapshot.message);

    report_export_result(&status, "OTLP trace export failed", &Ok(()));
    let snapshot = status.snapshot();
    assert!(!snapshot.degraded);
    assert!(snapshot.message.is_empty());
}

#[tokio::test]
async fn observability_status_transitions_notify_subscribers() {
    let status = ObservabilityStatus::default();
    let mut rx = status.subscribe();

    status.record_failure("link down");
    rx.changed().await.expect("failure transition");
    let snapshot = status.snapshot();
    assert!(snapshot.degraded);
    assert_eq!(snapshot.message, "link down");

    status.record_success();
    rx.changed().await.expect("recovery transition");
    let snapshot = status.snapshot();
    assert!(!snapshot.degraded);
    assert!(snapshot.message.is_empty());
}

#[tokio::test]
async fn from_config_records_exporter_construction_failure() {
    let handle = TelemetryHandle::from_config(TelemetryConfig {
        otlp_enabled: true,
        otlp_traces_endpoint: Some("http://[::1".into()),
        ..TelemetryConfig::default()
    })
    .await;

    let status = handle.status().snapshot();
    assert!(status.degraded, "{status:?}");
    assert!(
        status.message.contains("OpenTelemetry export is disabled"),
        "{status:?}"
    );

    handle.shutdown().await;
}

#[test]
fn full_content_observer_sets_langfuse_attributes_on_spans() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("theway-observability-content-test");
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
        full_content: true,
        status: Arc::new(ObservabilityStatus::default()),
    });
    assert!(observer.include_content());

    let parent = OperationScope::start(
        observer.clone(),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    let mut tool = OperationScope::start(
        observer,
        Some(parent.id()),
        ObservationContext::default().with_turn(1),
        OperationDetail::ToolExecution {
            tool_name: "bash".into(),
        },
    );
    tool.attach_content(ObservationContent {
        input: Some(serde_json::json!({ "command": "ls", "args": ["-la"] })),
        output: Some(serde_json::json!({ "stdout": "SECRET_TOOL_RESULT" })),
    });
    tool.finish(
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
    let agent = spans
        .iter()
        .find(|span| span.name == "agent.run")
        .expect("agent run span");
    let agent_trace_name = agent
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "langfuse.trace.name")
        .expect("root trace name");
    let opentelemetry::Value::String(trace_name) = &agent_trace_name.value else {
        panic!("trace name must be a string value");
    };
    assert_eq!(trace_name.as_ref(), "theway agent.run");

    let tool = spans
        .iter()
        .find(|span| span.name == "tool.execute")
        .expect("tool span");
    let observation_type = tool
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "langfuse.observation.type")
        .expect("observation type");
    assert!(format!("{:?}", observation_type.value).contains("span"));
    let input = tool
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "langfuse.observation.input")
        .expect("input content");
    assert!(format!("{:?}", input.value).contains("ls"));
    let output = tool
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "langfuse.observation.output")
        .expect("output content");
    assert!(format!("{:?}", output.value).contains("SECRET_TOOL_RESULT"));
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn prometheus_endpoint_has_bounded_labels_and_exact_measurements() {
    let handle = TelemetryHandle::from_config(TelemetryConfig {
        metrics_addr: Some("127.0.0.1:0".parse().unwrap()),
        queue_capacity: 16,
        ..TelemetryConfig::default()
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
    let completed = text.lines().find(|line| {
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
        line.starts_with("theway_runtime_tokens_total{") && line.contains("direction=\"input\"")
    });
    let output = text.lines().find(|line| {
        line.starts_with("theway_runtime_tokens_total{") && line.contains("direction=\"output\"")
    });
    assert!(input.is_some_and(|line| line.ends_with(" 11")));
    assert!(output.is_some_and(|line| line.ends_with(" 7")));
    for secret in ["secret-session-id", "secret-run-id"] {
        assert!(!text.contains(secret));
    }
    handle.shutdown().await;
}

#[test]
fn metric_context_from_detail_extracts_llm_and_compaction_fields() {
    let llm = MetricContext::from_detail(&OperationDetail::LlmRequest {
        provider: "provider-a".into(),
        model: "model-a".into(),
    });
    assert_eq!(llm.provider.as_deref(), Some("provider-a"));
    assert_eq!(llm.model.as_deref(), Some("model-a"));

    let compaction = MetricContext::from_detail(&OperationDetail::Compaction {
        algorithm: "map".into(),
        provider: "provider-b".into(),
        model: "model-b".into(),
    });
    assert_eq!(compaction.provider.as_deref(), Some("provider-b"));
    assert_eq!(compaction.model.as_deref(), Some("model-b"));

    let other = MetricContext::from_detail(&OperationDetail::ToolExecution {
        tool_name: "bash".into(),
    });
    assert!(other.provider.is_none());
    assert!(other.model.is_none());
}

#[test]
fn runtime_metrics_records_finish_without_started_entry() {
    let metrics = metrics();
    let scope = OperationScope::start(
        theway_core::noop_runtime_observer(),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    let id = scope.id();
    std::mem::forget(scope);
    let finish = OperationFinished {
        id,
        kind: OperationKind::AgentRun,
        context: ObservationContext::default(),
        outcome: OperationOutcome::Failed,
        error_category: Some(ErrorCategory::Tool),
        duration: Duration::from_millis(5),
        measurements: RuntimeMeasurements::default(),
        content: None,
    };
    metrics.record_finish(&finish, None);

    let families = metrics.prometheus.registry.gather();
    let operations = families
        .iter()
        .find(|family| family.name() == "theway_runtime_operations_total")
        .expect("operations metric");
    let metric = operations
        .get_metric()
        .iter()
        .find(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "operation" && label.value() == "agent.run")
                && metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "outcome" && label.value() == "failed")
                && metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "error_category" && label.value() == "tool")
        })
        .expect("failed tool operation metric");
    assert_eq!(metric.get_counter().value(), 1.0);
}

#[test]
fn worker_loop_records_all_operation_kinds_and_token_measurements() {
    let metrics = metrics();
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(64);
    let worker_stopped = stopped.clone();
    let worker_metrics = metrics.clone();
    let worker = std::thread::spawn(move || {
        worker_loop(rx, None, worker_metrics, worker_stopped);
    });
    let observer: Arc<dyn RuntimeObserver> = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: stopped.clone(),
        dropped: AtomicU64::new(0),
        metrics: metrics.clone(),
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
    });

    let details = [
        OperationDetail::AgentRun,
        OperationDetail::Turn { index: 0 },
        OperationDetail::LlmRequest {
            provider: "provider-a".into(),
            model: "model-a".into(),
        },
        OperationDetail::ToolExecution {
            tool_name: "bash".into(),
        },
        OperationDetail::Compaction {
            algorithm: "map".into(),
            provider: "provider-a".into(),
            model: "model-a".into(),
        },
        OperationDetail::SubagentJob {
            agent: "helper".into(),
            source: "test".into(),
        },
        OperationDetail::DagRun,
        OperationDetail::DagNode,
    ];
    for detail in details {
        let measurements = match detail.kind() {
            OperationKind::LlmRequest | OperationKind::Compaction => RuntimeMeasurements {
                input_tokens: 1,
                output_tokens: 2,
                cache_read_tokens: 3,
                cache_write_tokens: 4,
                characters: 5,
                turns: 6,
                tool_calls: 7,
            },
            _ => RuntimeMeasurements {
                characters: 5,
                turns: 6,
                tool_calls: 7,
                ..RuntimeMeasurements::default()
            },
        };
        OperationScope::start(
            observer.clone(),
            None,
            ObservationContext::default(),
            detail,
        )
        .finish(OperationOutcome::Succeeded, None, measurements);
    }
    stopped.store(true, Ordering::Release);
    worker.join().unwrap();

    let families = metrics.prometheus.registry.gather();
    let operations = families
        .iter()
        .find(|family| family.name() == "theway_runtime_operations_total")
        .expect("operations metric");
    for expected in [
        "agent.run",
        "agent.turn",
        "llm.request",
        "tool.execute",
        "session.compaction",
        "multiagent.job",
        "dag.run",
        "dag.node",
    ] {
        assert!(
            operations.get_metric().iter().any(|metric| {
                metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "operation" && label.value() == expected)
            }),
            "missing operation metric {expected}: {operations:?}"
        );
    }

    let tokens = families
        .iter()
        .find(|family| family.name() == "theway_runtime_tokens_total")
        .expect("tokens metric");
    assert!(
        tokens.get_metric().iter().any(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "direction" && label.value() == "input")
                && metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "provider" && label.value() == "provider-a")
        }),
        "missing LLM token metric: {tokens:?}"
    );
}

#[test]
fn worker_loop_records_abandoned_active_operations() {
    let metrics = metrics();
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(16);
    let worker_stopped = stopped.clone();
    let worker_metrics = metrics.clone();
    let worker = std::thread::spawn(move || {
        worker_loop(rx, None, worker_metrics, worker_stopped);
    });
    let observer: Arc<dyn RuntimeObserver> = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: stopped.clone(),
        dropped: AtomicU64::new(0),
        metrics: metrics.clone(),
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
    });
    let active = OperationScope::start(
        observer,
        None,
        ObservationContext::default(),
        OperationDetail::Turn { index: 0 },
    );
    std::mem::forget(active);

    stopped.store(true, Ordering::Release);
    worker.join().unwrap();

    let families = metrics.prometheus.registry.gather();
    let operations = families
        .iter()
        .find(|family| family.name() == "theway_runtime_operations_total")
        .expect("operations metric");
    assert!(
        operations.get_metric().iter().any(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "operation" && label.value() == "agent.turn")
                && metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "outcome" && label.value() == "abandoned")
                && metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == "error_category" && label.value() == "runtime")
        }),
        "missing abandoned metric: {operations:?}"
    );
}

#[test]
fn runtime_metrics_records_otel_paths_through_worker() {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter).build())
        .build();
    let meter = provider.meter("theway-observability-otel-test");
    let metrics = Arc::new(RuntimeMetrics {
        prometheus: PrometheusMetrics::new(),
        otel: Some(OtelMetrics::new(&meter)),
    });
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(16);
    let worker_stopped = stopped.clone();
    let worker_metrics = metrics.clone();
    let worker = std::thread::spawn(move || {
        worker_loop(rx, None, worker_metrics, worker_stopped);
    });
    let observer: Arc<dyn RuntimeObserver> = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: stopped.clone(),
        dropped: AtomicU64::new(0),
        metrics: metrics.clone(),
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
    });

    OperationScope::start(
        observer.clone(),
        None,
        ObservationContext::default(),
        OperationDetail::LlmRequest {
            provider: "provider-otel".into(),
            model: "model-otel".into(),
        },
    )
    .finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 4,
            characters: 5,
            turns: 6,
            tool_calls: 7,
        },
    );
    let abandoned = OperationScope::start(
        observer,
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    std::mem::forget(abandoned);

    stopped.store(true, Ordering::Release);
    worker.join().unwrap();
    provider.shutdown().unwrap();
}

#[test]
fn telemetry_config_from_env_detects_otlp_metrics_addr_and_queue_capacity() {
    let _guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set(
        "THEWAY_DIR",
        "/tmp/theway-observability-env-tests",
    );
    let _endpoint = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_ENDPOINT", "");
    let _traces = crate::test_env::EnvGuard::set(
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "http://127.0.0.1:4317",
    );
    let _headers = crate::test_env::EnvGuard::set(
        "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
        "Authorization=Basic dGVzdA==",
    );
    let _metrics = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "");
    let _addr = crate::test_env::EnvGuard::set("THEWAY_METRICS_ADDR", "127.0.0.1:9876");
    let _queue = crate::test_env::EnvGuard::set("THEWAY_OBSERVABILITY_QUEUE_CAPACITY", "123");
    let _content = crate::test_env::EnvGuard::set("THEWAY_OBSERVABILITY_FULL_CONTENT", "true");

    let config = TelemetryConfig::from_env();

    assert!(config.otlp_enabled);
    assert!(!config.otlp_metrics_enabled);
    assert_eq!(
        config.otlp_traces_endpoint.as_deref(),
        Some("http://127.0.0.1:4317")
    );
    assert_eq!(
        config
            .otlp_traces_headers
            .as_ref()
            .and_then(|headers| headers.get("Authorization"))
            .map(String::as_str),
        Some("Basic dGVzdA==")
    );
    assert_eq!(config.otlp_metrics_endpoint, None);
    assert_eq!(config.metrics_addr, Some("127.0.0.1:9876".parse().unwrap()));
    assert_eq!(config.queue_capacity, 123);
    assert!(config.full_content);
}

#[test]
fn telemetry_config_from_env_enables_metrics_exporter_with_metrics_endpoint() {
    let _guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set(
        "THEWAY_DIR",
        "/tmp/theway-observability-env-tests",
    );
    let _endpoint = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_ENDPOINT", "");
    let _traces = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "");
    let _metrics = crate::test_env::EnvGuard::set(
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        "http://127.0.0.1:4317",
    );

    let config = TelemetryConfig::from_env();

    assert!(!config.otlp_enabled);
    assert!(config.otlp_metrics_enabled);
    assert_eq!(
        config.otlp_metrics_endpoint.as_deref(),
        Some("http://127.0.0.1:4317")
    );
}

#[test]
fn telemetry_config_from_env_ignores_invalid_values() {
    let _guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set(
        "THEWAY_DIR",
        "/tmp/theway-observability-env-tests",
    );
    let _endpoint = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_ENDPOINT", "");
    let _traces = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "");
    let _metrics = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "");
    let _addr = crate::test_env::EnvGuard::set("THEWAY_METRICS_ADDR", "not-an-address");
    let _queue = crate::test_env::EnvGuard::set("THEWAY_OBSERVABILITY_QUEUE_CAPACITY", "0");
    let _content = crate::test_env::EnvGuard::set("THEWAY_OBSERVABILITY_FULL_CONTENT", "banana");

    let config = TelemetryConfig::from_env();

    assert!(!config.otlp_enabled);
    assert!(!config.otlp_metrics_enabled);
    assert_eq!(config.otlp_traces_endpoint, None);
    assert_eq!(config.otlp_metrics_endpoint, None);
    assert_eq!(config.metrics_addr, None);
    assert_eq!(config.queue_capacity, DEFAULT_QUEUE_CAPACITY);
    assert!(!config.full_content);
}

#[test]
fn telemetry_config_reads_observability_env_file_as_fallback() {
    let _guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("observability.env"),
        "# langfuse\n\
         OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=\"https://lf.example/api/public/otel\"\n\
         OTEL_EXPORTER_OTLP_TRACES_HEADERS=Authorization=Basic dGVzdA==\n\
         THEWAY_OBSERVABILITY_FULL_CONTENT=true\n",
    )
    .unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", temp.path().to_str().unwrap());
    let _endpoint = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_ENDPOINT", "");
    let _traces = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "");
    let _headers = crate::test_env::EnvGuard::set("OTEL_EXPORTER_OTLP_TRACES_HEADERS", "");
    let _content = crate::test_env::EnvGuard::set("THEWAY_OBSERVABILITY_FULL_CONTENT", "");

    let config = TelemetryConfig::from_env();

    assert!(config.otlp_enabled);
    assert!(!config.otlp_metrics_enabled);
    assert_eq!(
        config.otlp_traces_endpoint.as_deref(),
        Some("https://lf.example/api/public/otel")
    );
    assert_eq!(
        config
            .otlp_traces_headers
            .as_ref()
            .and_then(|headers| headers.get("Authorization"))
            .map(String::as_str),
        Some("Basic dGVzdA==")
    );
    assert!(config.full_content);
}

#[test]
fn worker_loop_trace_attributes_cover_all_details_and_error_status() {
    use opentelemetry::trace::Status;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("theway-observability-trace-attributes-test");
    let metrics = metrics();
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(64);
    let worker_stopped = stopped.clone();
    let worker_metrics = metrics.clone();
    let worker = std::thread::spawn(move || {
        worker_loop(rx, Some(tracer), worker_metrics, worker_stopped);
    });
    let observer: Arc<dyn RuntimeObserver> = Arc::new(DaemonRuntimeObserver {
        tx,
        stopped: stopped.clone(),
        dropped: AtomicU64::new(0),
        metrics: metrics.clone(),
        full_content: false,
        status: Arc::new(ObservabilityStatus::default()),
    });
    let context = ObservationContext {
        session_id: Some("session-a".into()),
        run_id: Some("run-a".into()),
        turn_id: Some(1),
        job_id: Some("job-a".into()),
        node_id: Some("node-a".into()),
    };

    let cases = [
        (
            OperationDetail::Turn { index: 2 },
            OperationOutcome::Succeeded,
            None,
            RuntimeMeasurements::default(),
        ),
        (
            OperationDetail::LlmRequest {
                provider: "provider-a".into(),
                model: "model-a".into(),
            },
            OperationOutcome::Failed,
            Some(ErrorCategory::Provider),
            RuntimeMeasurements {
                input_tokens: 10,
                output_tokens: 20,
                ..RuntimeMeasurements::default()
            },
        ),
        (
            OperationDetail::ToolExecution {
                tool_name: "bash".into(),
            },
            OperationOutcome::TimedOut,
            Some(ErrorCategory::Timeout),
            RuntimeMeasurements {
                tool_calls: 1,
                ..RuntimeMeasurements::default()
            },
        ),
        (
            OperationDetail::Compaction {
                algorithm: "map".into(),
                provider: "provider-b".into(),
                model: "model-b".into(),
            },
            OperationOutcome::Cancelled,
            Some(ErrorCategory::Cancellation),
            RuntimeMeasurements {
                cache_read_tokens: 7,
                ..RuntimeMeasurements::default()
            },
        ),
        (
            OperationDetail::SubagentJob {
                agent: "helper".into(),
                source: "test".into(),
            },
            OperationOutcome::Succeeded,
            None,
            RuntimeMeasurements::default(),
        ),
        (
            OperationDetail::DagRun,
            OperationOutcome::Succeeded,
            None,
            RuntimeMeasurements::default(),
        ),
        (
            OperationDetail::DagNode,
            OperationOutcome::Failed,
            Some(ErrorCategory::Runtime),
            RuntimeMeasurements::default(),
        ),
    ];
    for (detail, outcome, category, measurements) in cases {
        OperationScope::start(observer.clone(), None, context.clone(), detail).finish(
            outcome,
            category,
            measurements,
        );
    }
    stopped.store(true, Ordering::Release);
    worker.join().unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    let llm = spans
        .iter()
        .find(|span| span.name == "llm.request")
        .expect("llm.request span");
    assert!(llm.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "gen_ai.provider.name" && attribute.value.as_str() == "provider-a"
    }));
    assert!(llm.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "theway.session.id" && attribute.value.as_str() == "session-a"
    }));
    assert!(llm.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "theway.turn.id" && attribute.value.as_str() == "1"
    }));
    assert_eq!(llm.status, Status::error("failed"));

    let tool = spans
        .iter()
        .find(|span| span.name == "tool.execute")
        .expect("tool.execute span");
    assert!(tool.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "theway.tool.name" && attribute.value.as_str() == "bash"
    }));
    assert!(tool.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "error.type" && attribute.value.as_str() == "timeout"
    }));
    assert_eq!(tool.status, Status::error("timed_out"));

    let compaction = spans
        .iter()
        .find(|span| span.name == "session.compaction")
        .expect("session.compaction span");
    assert!(compaction.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "theway.compaction.algorithm" && attribute.value.as_str() == "map"
    }));
    assert!(compaction.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "gen_ai.request.model" && attribute.value.as_str() == "model-b"
    }));

    let subagent = spans
        .iter()
        .find(|span| span.name == "multiagent.job")
        .expect("multiagent.job span");
    assert!(subagent.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "theway.agent.name" && attribute.value.as_str() == "helper"
    }));
    assert!(subagent.attributes.iter().any(|attribute| {
        attribute.key.as_str() == "theway.agent.source" && attribute.value.as_str() == "test"
    }));

    assert!(spans.iter().any(|span| span.name == "dag.run"));
    assert!(spans.iter().any(|span| span.name == "dag.node"));
    provider.shutdown().unwrap();
}
