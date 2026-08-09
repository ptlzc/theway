//! OTLP HTTP/JSON span exporter. Hand-rolled tracing-subscriber Layer that buffers spans on
//! close and POSTs OTLP-shaped JSON to `${OTEL_EXPORTER_OTLP_ENDPOINT}/v1/traces` in batches.
//!
//! Closes the OTLP slot of c4pt0r/theway#15. Activates automatically when
//! `OTEL_EXPORTER_OTLP_ENDPOINT` is set; silent no-op otherwise.
//!
//! Why hand-rolled: the official `opentelemetry` + `tracing-opentelemetry` crates have a
//! version-churn history that complicates pinning. The OTLP/JSON wire format is small enough
//! to encode by hand, and we don't need the full OTel SDK (metrics, log signal, propagators)
//! for a coding agent.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::{Value, json};
use tracing::span;
use tracing::subscriber::Interest;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

const BATCH_SIZE: usize = 64;
const FLUSH_INTERVAL_MS: u64 = 2_000;

/// Try to build an OTLP layer from `OTEL_EXPORTER_OTLP_ENDPOINT`. Returns `None` when the env
/// var isn't set so the caller can skip installation cleanly.
pub fn try_layer() -> Option<OtlpLayer> {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok()?;
    if endpoint.trim().is_empty() {
        return None;
    }
    Some(OtlpLayer::new(endpoint))
}

#[derive(Clone)]
pub struct OtlpLayer {
    inner: Arc<Inner>,
}

struct Inner {
    endpoint: String,
    service_name: String,
    /// In-memory ring of finished spans waiting for the next batch flush.
    pending: Mutex<Vec<Value>>,
    /// Per-span lookaside that holds opened-but-not-yet-closed spans so we can compute their
    /// duration on close. Keyed by tracing span id.
    open: Mutex<HashMap<u64, OpenSpan>>,
}

struct OpenSpan {
    name: String,
    target: String,
    start_ns: u128,
    attributes: HashMap<String, String>,
}

impl OtlpLayer {
    pub fn new(endpoint: impl Into<String>) -> Self {
        let inner = Arc::new(Inner {
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            service_name: "theway".into(),
            pending: Mutex::new(Vec::new()),
            open: Mutex::new(HashMap::new()),
        });
        let pumper = inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(FLUSH_INTERVAL_MS)).await;
                Self::flush_once(&pumper).await;
            }
        });
        Self { inner }
    }

    pub fn with_service_name(self, name: impl Into<String>) -> Self {
        // The Arc<Inner> is shared; create a new wrapper that points at a fresh inner if the
        // caller wants a different service name. Cheaper to just allow this once at construction.
        let mut inner = (*self.inner).clone_for_rename();
        inner.service_name = name.into();
        Self {
            inner: Arc::new(inner),
        }
    }

    async fn flush_once(inner: &Arc<Inner>) {
        let drained: Vec<Value> = {
            let mut g = inner.pending.lock();
            if g.is_empty() {
                return;
            }
            std::mem::take(&mut *g)
        };
        let payload = json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        { "key": "service.name", "value": { "stringValue": inner.service_name } },
                        { "key": "service.version", "value": { "stringValue": env!("CARGO_PKG_VERSION") } }
                    ]
                },
                "scopeSpans": [{
                    "scope": { "name": "theway" },
                    "spans": drained
                }]
            }]
        });
        let endpoint = format!("{}/v1/traces", inner.endpoint);
        // Fire and forget — OTLP collectors are advisory, never load-bearing for the agent.
        if let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        {
            let req = client.post(&endpoint).json(&payload);
            tokio::spawn(async move {
                let _ = req.send().await;
            });
        }
    }
}

impl Inner {
    /// Re-bind the service name without poisoning the existing inner's queue. Used only by
    /// `with_service_name`.
    fn clone_for_rename(&self) -> Inner {
        Inner {
            endpoint: self.endpoint.clone(),
            service_name: self.service_name.clone(),
            pending: Mutex::new(Vec::new()),
            open: Mutex::new(HashMap::new()),
        }
    }
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

impl<S> Layer<S> for OtlpLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(&self, _meta: &'static tracing::Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, _ctx: Context<'_, S>) {
        let mut visitor = AttrCollector::default();
        attrs.record(&mut visitor);
        self.inner.open.lock().insert(
            id.into_u64(),
            OpenSpan {
                name: attrs.metadata().name().to_string(),
                target: attrs.metadata().target().to_string(),
                start_ns: now_ns(),
                attributes: visitor.attrs,
            },
        );
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        let removed = self.inner.open.lock().remove(&id.into_u64());
        let Some(span) = removed else { return };
        let end_ns = now_ns();
        let attrs_json: Vec<Value> = span
            .attributes
            .iter()
            .map(|(k, v)| json!({ "key": k, "value": { "stringValue": v } }))
            .collect();
        // OTLP span id / trace id are 8 / 16 hex bytes. We don't have a propagation source so
        // synthesize per-span. This keeps spans queryable but they aren't linked into a
        // distributed trace yet.
        let mut all_attrs: Vec<Value> = vec![json!({
            "key": "tracing.target",
            "value": { "stringValue": span.target }
        })];
        all_attrs.extend(attrs_json);
        let span_obj = json!({
            "traceId": hex_random(16),
            "spanId": hex_random(8),
            "name": span.name,
            "kind": 1,
            "startTimeUnixNano": span.start_ns.to_string(),
            "endTimeUnixNano": end_ns.to_string(),
            "attributes": all_attrs,
            "status": { "code": 1 }
        });
        let mut g = self.inner.pending.lock();
        g.push(span_obj);
        if g.len() >= BATCH_SIZE {
            // Defer the actual POST; flush_once runs on the next tick.
            // (No tokio::spawn here — we're inside a sync trait method.)
        }
    }
}

fn hex_random(bytes: usize) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let mut s = SEED.fetch_add(0x6364_1362_2384_6793, Ordering::Relaxed);
    let mut out = String::with_capacity(bytes * 2);
    for _ in 0..bytes {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.push_str(&format!("{:02x}", (s >> 56) as u8));
    }
    out
}

#[derive(Default)]
struct AttrCollector {
    attrs: HashMap<String, String>,
}

impl tracing::field::Visit for AttrCollector {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.attrs
            .insert(field.name().to_string(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.attrs
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.attrs
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.attrs
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.attrs
            .insert(field.name().to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_layer_returns_none_when_env_unset() {
        // SAFETY: tests share process state; explicitly clear the env var so this is
        // deterministic.
        unsafe { std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT") };
        assert!(try_layer().is_none());
    }

    #[test]
    fn hex_random_returns_correct_length() {
        let a = hex_random(8);
        assert_eq!(a.len(), 16);
        let b = hex_random(16);
        assert_eq!(b.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
