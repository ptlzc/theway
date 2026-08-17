//! Tests for `otlp` — split out of src (see docs/rust-test-files.md).
//!
//! These exercise the private layer internals (open-span bookkeeping, pending
//! batch, flush) without a real OTLP collector: `flush_once` is pointed at an
//! unroutable endpoint and the span-recording tests only inspect the in-memory
//! ring.

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt as _;

#[tokio::test]
async fn layer_new_trims_endpoint_and_starts_with_empty_buffers() {
    let layer = OtlpLayer::new("http://localhost:4318/");

    assert_eq!(layer.inner.endpoint, "http://localhost:4318");
    assert_eq!(layer.inner.service_name, "theway");
    assert!(layer.inner.pending.lock().is_empty());
    assert!(layer.inner.open.lock().is_empty());
}

#[test]
fn hex_random_returns_requested_bytes_and_hex_chars() {
    let empty = hex_random(0);
    assert!(empty.is_empty());

    let a = hex_random(8);
    let b = hex_random(8);
    assert_eq!(a.len(), 16);
    assert_eq!(b.len(), 16);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn now_ns_is_monotonic_epoch_time() {
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let got = now_ns();
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    assert!(got >= before && got <= after);
}

#[tokio::test]
async fn layer_records_span_attributes_on_open_and_close() {
    // Arrange: a registry subscriber carrying our layer.
    let layer = OtlpLayer::new("http://127.0.0.1:4318");
    let subscriber = Registry::default().with(layer.clone());

    // Act: create and drop a span inside the subscriber context.
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(target: "otlp-test", "test_span", foo = "bar", n = 42_i64, flag = true);
        let _guard = span.enter();
    });

    // Assert: opened span moved from `open` into `pending` with attributes.
    assert!(layer.inner.open.lock().is_empty());
    let pending = layer.inner.pending.lock();
    assert_eq!(pending.len(), 1);
    let span_obj = &pending[0];
    assert_eq!(span_obj["name"], "test_span");
    assert_eq!(span_obj["kind"], 1);
    assert_eq!(span_obj["status"]["code"], 1);
    assert_eq!(span_obj["traceId"].as_str().unwrap().len(), 32);
    assert_eq!(span_obj["spanId"].as_str().unwrap().len(), 16);

    let attrs = span_obj["attributes"].as_array().unwrap();
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "tracing.target" && a["value"]["stringValue"] == "otlp-test")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "foo" && a["value"]["stringValue"] == "bar")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "n" && a["value"]["stringValue"] == "42")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "flag" && a["value"]["stringValue"] == "true")
    );
}

#[tokio::test]
async fn flush_once_drains_pending_span_batch() {
    let layer = OtlpLayer::new("http://127.0.0.1:9");
    layer.inner.pending.lock().push(json!({
        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "spanId": "bbbbbbbbbbbbbbbb",
        "name": "flush-me",
        "kind": 1,
        "startTimeUnixNano": "0",
        "endTimeUnixNano": "1",
        "attributes": [],
        "status": { "code": 1 }
    }));

    // Act: flush the pending batch (POST is fire-and-forget; no collector needed).
    OtlpLayer::flush_once(&layer.inner).await;

    // Assert: the batch is drained even though the HTTP response is ignored.
    assert!(layer.inner.pending.lock().is_empty());
}

#[tokio::test]
async fn flush_once_leaves_empty_pending_untouched() {
    let layer = OtlpLayer::new("http://127.0.0.1:9");

    OtlpLayer::flush_once(&layer.inner).await;

    assert!(layer.inner.pending.lock().is_empty());
}
