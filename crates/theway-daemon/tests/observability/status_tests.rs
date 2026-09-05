use super::*;
use super::super::status::report_export_result;

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
