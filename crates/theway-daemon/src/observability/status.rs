use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::Temporality;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::trace::SpanData;
use tokio::sync::watch;

/// Presentation snapshot of the daemon observer health. Recorded by exporter
/// wrappers, exporter construction, and queue-drop accounting; consumed by the
/// transport status plane so the TUI can show a hint. Never affects agent
/// control flow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservabilityStatusSnapshot {
    pub degraded: bool,
    pub message: String,
}

/// Shared, thread-safe observer health handle. Exporter errors arrive on the
/// SDK's blocking batch threads, so all mutation is synchronous and lock-free
/// where possible; `watch` wakes the daemon event loop to republish the status.
#[derive(Clone, Debug)]
pub struct ObservabilityStatus {
    inner: Arc<ObservabilityStatusInner>,
}

#[derive(Debug)]
struct ObservabilityStatusInner {
    state: parking_lot::Mutex<ObservabilityStatusSnapshot>,
    revision: AtomicU64,
    notify: watch::Sender<u64>,
}

impl Default for ObservabilityStatus {
    fn default() -> Self {
        let (notify, _) = watch::channel(0);
        Self {
            inner: Arc::new(ObservabilityStatusInner {
                state: parking_lot::Mutex::new(ObservabilityStatusSnapshot::default()),
                revision: AtomicU64::new(0),
                notify,
            }),
        }
    }
}

impl ObservabilityStatus {
    pub fn record_failure(&self, message: impl Into<String>) {
        let message = message.into();
        {
            let mut state = self.inner.state.lock();
            if state.degraded && state.message == message {
                return;
            }
            state.degraded = true;
            state.message = message;
        }
        self.bump();
    }

    /// Clear a previously-recorded failure. Exporters call this after a
    /// successful send so the hint disappears once the link recovers.
    pub fn record_success(&self) {
        {
            let mut state = self.inner.state.lock();
            if !state.degraded {
                return;
            }
            state.degraded = false;
            state.message.clear();
        }
        self.bump();
    }

    pub fn snapshot(&self) -> ObservabilityStatusSnapshot {
        self.inner.state.lock().clone()
    }

    /// Receives a new value on every failure/success transition.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.notify.subscribe()
    }

    fn bump(&self) {
        let revision = self.inner.revision.fetch_add(1, Ordering::Release) + 1;
        let _ = self.inner.notify.send_replace(revision);
    }
}

/// Span exporter wrapper that records the latest OTLP send result into the
/// shared observer status (success clears a previous failure).
#[derive(Debug)]
pub(super) struct StatusReportingSpanExporter {
    pub(super) inner: opentelemetry_otlp::SpanExporter,
    pub(super) status: Arc<ObservabilityStatus>,
}

impl opentelemetry_sdk::trace::SpanExporter for StatusReportingSpanExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let result = self.inner.export(batch).await;
        report_export_result(&self.status, "OTLP trace export failed", &result);
        result
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&mut self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

/// Metric exporter wrapper with the same status reporting behavior.
#[derive(Debug)]
pub(super) struct StatusReportingMetricExporter {
    pub(super) inner: opentelemetry_otlp::MetricExporter,
    pub(super) status: Arc<ObservabilityStatus>,
}

impl PushMetricExporter for StatusReportingMetricExporter {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let result = self.inner.export(metrics).await;
        report_export_result(&self.status, "OTLP metric export failed", &result);
        result
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn temporality(&self) -> Temporality {
        self.inner.temporality()
    }
}

pub(super) fn report_export_result(
    status: &Arc<ObservabilityStatus>,
    prefix: &str,
    result: &OTelSdkResult,
) {
    match result {
        Ok(()) => status.record_success(),
        Err(error) => status.record_failure(format!("{prefix}: {error}")),
    }
}
