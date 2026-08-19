use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter, UpDownCounter};
use prometheus::{HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGaugeVec};
use theway_core::{
    ErrorCategory, OperationFinished, OperationKind, OperationOutcome, RuntimeMeasurements,
};

use super::{ActiveOperation, MetricContext};

pub(super) struct RuntimeMetrics {
    pub(super) prometheus: PrometheusMetrics,
    pub(super) otel: Option<OtelMetrics>,
}

impl RuntimeMetrics {
    pub(super) fn record_start(&self, kind: OperationKind) {
        let operation = kind.as_str();
        self.prometheus.active.with_label_values(&[operation]).inc();
        if let Some(otel) = &self.otel {
            otel.active.add(1, &[KeyValue::new("operation", operation)]);
        }
    }

    pub(super) fn record_finish(
        &self,
        finish: &OperationFinished,
        started: Option<&ActiveOperation>,
    ) {
        let operation = finish.kind.as_str();
        let outcome = finish.outcome.as_str();
        let error_category = finish
            .error_category
            .map(ErrorCategory::as_str)
            .unwrap_or("none");
        if started.is_some() {
            self.prometheus.active.with_label_values(&[operation]).dec();
        }
        self.prometheus
            .operations
            .with_label_values(&[operation, outcome, error_category])
            .inc();
        self.prometheus
            .duration
            .with_label_values(&[operation, outcome, error_category])
            .observe(finish.duration.as_secs_f64());
        record_measurements(
            &self.prometheus,
            finish.kind,
            started.map(|operation| &operation.metric_context),
            finish.measurements,
        );
        if let Some(otel) = &self.otel {
            let labels = [
                KeyValue::new("operation", operation),
                KeyValue::new("outcome", outcome),
                KeyValue::new("error_category", error_category),
            ];
            if started.is_some() {
                otel.active
                    .add(-1, &[KeyValue::new("operation", operation)]);
            }
            otel.operations.add(1, &labels);
            otel.duration.record(finish.duration.as_secs_f64(), &labels);
            otel_record_measurements(
                otel,
                finish.kind,
                started.map(|operation| &operation.metric_context),
                finish.measurements,
            );
        }
    }

    pub(super) fn record_abandoned(&self, kind: OperationKind) {
        let operation = kind.as_str();
        let outcome = OperationOutcome::Abandoned.as_str();
        let error_category = ErrorCategory::Runtime.as_str();
        self.prometheus.active.with_label_values(&[operation]).dec();
        self.prometheus
            .operations
            .with_label_values(&[operation, outcome, error_category])
            .inc();
        self.prometheus
            .duration
            .with_label_values(&[operation, outcome, error_category])
            .observe(0.0);
        if let Some(otel) = &self.otel {
            let labels = [
                KeyValue::new("operation", operation),
                KeyValue::new("outcome", outcome),
                KeyValue::new("error_category", error_category),
            ];
            otel.active
                .add(-1, &[KeyValue::new("operation", operation)]);
            otel.operations.add(1, &labels);
            otel.duration.record(0.0, &labels);
        }
    }

    pub(super) fn record_drop(&self) {
        self.prometheus.dropped.inc();
        if let Some(otel) = &self.otel {
            otel.dropped.add(1, &[]);
        }
    }
}

pub(super) struct OtelMetrics {
    operations: Counter<u64>,
    duration: Histogram<f64>,
    measurements: Counter<u64>,
    tokens: Counter<u64>,
    active: UpDownCounter<i64>,
    dropped: Counter<u64>,
}

impl OtelMetrics {
    pub(super) fn new(meter: &Meter) -> Self {
        Self {
            operations: meter.u64_counter("theway.runtime.operations").build(),
            duration: meter
                .f64_histogram("theway.runtime.operation.duration")
                .build(),
            measurements: meter.u64_counter("theway.runtime.measurements").build(),
            tokens: meter.u64_counter("theway.runtime.tokens").build(),
            active: meter.i64_up_down_counter("theway.runtime.active").build(),
            dropped: meter
                .u64_counter("theway.runtime.observations.dropped")
                .build(),
        }
    }
}

pub(super) struct PrometheusMetrics {
    pub(super) registry: prometheus::Registry,
    operations: IntCounterVec,
    duration: HistogramVec,
    measurements: IntCounterVec,
    tokens: IntCounterVec,
    active: IntGaugeVec,
    dropped: IntCounter,
}

impl PrometheusMetrics {
    pub(super) fn new() -> Self {
        let registry = prometheus::Registry::new();
        let operations = IntCounterVec::new(
            prometheus::Opts::new("theway_runtime_operations_total", "Completed operations"),
            &["operation", "outcome", "error_category"],
        )
        .expect("static Prometheus operation metric");
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "theway_runtime_operation_duration_seconds",
                "Operation duration",
            ),
            &["operation", "outcome", "error_category"],
        )
        .expect("static Prometheus duration metric");
        let measurements = IntCounterVec::new(
            prometheus::Opts::new(
                "theway_runtime_measurements_total",
                "Runtime token and activity measurements",
            ),
            &["operation", "measurement"],
        )
        .expect("static Prometheus measurement metric");
        let tokens = IntCounterVec::new(
            prometheus::Opts::new("theway_runtime_tokens_total", "LLM token usage"),
            &["provider", "model", "direction"],
        )
        .expect("static Prometheus token metric");
        let active = IntGaugeVec::new(
            prometheus::Opts::new("theway_runtime_active", "Active operations"),
            &["operation"],
        )
        .expect("static Prometheus active metric");
        let dropped = IntCounter::new(
            "theway_runtime_observations_dropped_total",
            "Observations dropped before export",
        )
        .expect("static Prometheus dropped metric");
        for collector in [
            Box::new(operations.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(duration.clone()),
            Box::new(measurements.clone()),
            Box::new(tokens.clone()),
            Box::new(active.clone()),
            Box::new(dropped.clone()),
        ] {
            registry
                .register(collector)
                .expect("unique static Prometheus metric");
        }
        Self {
            registry,
            operations,
            duration,
            measurements,
            tokens,
            active,
            dropped,
        }
    }

    pub(super) fn registry(&self) -> prometheus::Registry {
        self.registry.clone()
    }
}

fn record_measurements(
    metrics: &PrometheusMetrics,
    kind: OperationKind,
    context: Option<&MetricContext>,
    value: RuntimeMeasurements,
) {
    let operation = kind.as_str();
    for (measurement, amount) in activity_values(value) {
        if amount > 0 {
            metrics
                .measurements
                .with_label_values(&[operation, measurement])
                .inc_by(amount);
        }
    }
    if matches!(kind, OperationKind::LlmRequest | OperationKind::Compaction) {
        let provider = context
            .and_then(|context| context.provider.as_deref())
            .unwrap_or("unknown");
        let model = context
            .and_then(|context| context.model.as_deref())
            .unwrap_or("unknown");
        for (direction, amount) in token_values(value) {
            if amount > 0 {
                metrics
                    .tokens
                    .with_label_values(&[provider, model, direction])
                    .inc_by(amount);
            }
        }
    }
}

fn otel_record_measurements(
    metrics: &OtelMetrics,
    kind: OperationKind,
    context: Option<&MetricContext>,
    value: RuntimeMeasurements,
) {
    let operation = kind.as_str();
    for (measurement, amount) in activity_values(value) {
        if amount > 0 {
            metrics.measurements.add(
                amount,
                &[
                    KeyValue::new("operation", operation.to_string()),
                    KeyValue::new("measurement", measurement),
                ],
            );
        }
    }
    if matches!(kind, OperationKind::LlmRequest | OperationKind::Compaction) {
        let provider = context
            .and_then(|context| context.provider.as_deref())
            .unwrap_or("unknown");
        let model = context
            .and_then(|context| context.model.as_deref())
            .unwrap_or("unknown");
        for (direction, amount) in token_values(value) {
            if amount > 0 {
                metrics.tokens.add(
                    amount,
                    &[
                        KeyValue::new("provider", provider.to_string()),
                        KeyValue::new("model", model.to_string()),
                        KeyValue::new("direction", direction),
                    ],
                );
            }
        }
    }
}

fn activity_values(value: RuntimeMeasurements) -> [(&'static str, u64); 3] {
    [
        ("characters", value.characters),
        ("turns", value.turns),
        ("tool_calls", value.tool_calls),
    ]
}

fn token_values(value: RuntimeMeasurements) -> [(&'static str, u64); 4] {
    [
        ("input", value.input_tokens),
        ("output", value.output_tokens),
        ("cache_read", value.cache_read_tokens),
        ("cache_write", value.cache_write_tokens),
    ]
}
