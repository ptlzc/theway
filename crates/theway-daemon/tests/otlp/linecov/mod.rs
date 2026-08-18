//! Line-coverage completion tests for `otlp`.
//!
//! These close the remaining uncovered lines in the hand-rolled OTLP layer:
//! the background flush loop spawned by [`OtlpLayer::new`] and the
//! `record_debug` / `record_u64` arms of the span-attribute visitor.

use std::time::Duration;

use serde_json::json;
use tokio::io::AsyncReadExt as _;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt as _;

use super::super::*;

#[tokio::test]
async fn attr_collector_records_debug_and_u64_fields() {
    // Arrange: a subscriber carrying the layer.
    let layer = OtlpLayer::new("http://127.0.0.1:4318");
    let subscriber = Registry::default().with(layer.clone());

    // Act: create a span with a debug-recorded value and an unsigned integer.
    let debug_value = 42_u8;
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            target: "otlp-linecov",
            "debug_u64_span",
            debug_value = ?debug_value,
            str_value = "hello",
            signed = 7_i64,
            unsigned = 7_u64,
            flag = true
        );
        let _guard = span.enter();
    });

    // Assert: every field was captured with the expected string representation.
    let pending = layer.inner.pending.lock();
    assert_eq!(pending.len(), 1);
    let attrs = pending[0]["attributes"].as_array().unwrap();
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "debug_value" && a["value"]["stringValue"] == "42")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "str_value" && a["value"]["stringValue"] == "hello")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "signed" && a["value"]["stringValue"] == "7")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "unsigned" && a["value"]["stringValue"] == "7")
    );
    assert!(
        attrs
            .iter()
            .any(|a| a["key"] == "flag" && a["value"]["stringValue"] == "true")
    );
}

#[tokio::test]
async fn background_flusher_drains_pending_after_interval_and_posts() {
    // Arrange: a local listener receives the POST fired by the background loop.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let layer = OtlpLayer::new(format!("http://{addr}"));
    layer.inner.pending.lock().push(json!({
        "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "spanId": "bbbbbbbbbbbbbbbb",
        "name": "background-flush",
        "kind": 1,
        "startTimeUnixNano": "0",
        "endTimeUnixNano": "1",
        "attributes": [],
        "status": { "code": 1 }
    }));

    let accept_task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("timed out waiting for OTLP POST")
            .unwrap();
        let mut buf = [0_u8; 1024];
        let _ = socket.read(&mut buf).await;
    });

    // Act: let the background flusher reach its 2 s tick and run flush_once.
    tokio::time::sleep(Duration::from_millis(2_100)).await;

    // Assert: the pending batch was drained and the POST reached our listener.
    accept_task.await.unwrap();
    assert!(layer.inner.pending.lock().is_empty());
}
