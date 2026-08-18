//! Additional mirrored coverage for `otlp` — `try_layer` env handling and
//! multi-span batch draining.

use serde_json::json;

use super::super::*;
use crate::test_env::{EnvGuard, ENV_LOCK};

#[test]
fn try_layer_returns_none_for_empty_endpoint() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::set("OTEL_EXPORTER_OTLP_ENDPOINT", "   ");

    assert!(try_layer().is_none());
}

#[tokio::test]
async fn try_layer_returns_some_and_trims_endpoint() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::set("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318/");

    let layer = try_layer().expect("endpoint is set");
    assert_eq!(layer.inner.endpoint, "http://localhost:4318");
}

#[tokio::test]
async fn flush_once_drains_multiple_pending_spans() {
    let layer = OtlpLayer::new("http://127.0.0.1:9");
    for i in 0..3 {
        layer.inner.pending.lock().push(json!({
            "traceId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "spanId": format!("bbbbbbbbbbbbbb{i:02x}"),
            "name": format!("span-{i}"),
            "kind": 1,
            "startTimeUnixNano": "0",
            "endTimeUnixNano": "1",
            "attributes": [],
            "status": { "code": 1 }
        }));
    }

    OtlpLayer::flush_once(&layer.inner).await;

    assert!(layer.inner.pending.lock().is_empty());
}
