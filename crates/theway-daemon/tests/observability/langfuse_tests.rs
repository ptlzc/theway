use super::*;

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
