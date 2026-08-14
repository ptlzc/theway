use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Test sink recording dirty notifications; flush is a no-op snapshot.
struct CountingSink {
    count: AtomicUsize,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl DagPersistSink for CountingSink {
    fn notify_dirty(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
    async fn flush(&self) {}
}

/// Engine state changes must notify the persist sink: plan, node completion,
/// run completion, and cancel all fire `notify_dirty`.
#[test]
fn state_changes_notify_persist_sink() {
    let engine = DagEngine::new();
    let sink = Arc::new(CountingSink::new());
    engine.set_persist_sink(Some(sink.clone() as Arc<dyn DagPersistSink>));

    // plan fires
    let run_id = engine.plan(
        DagRunDef {
            name: "p".into(),
            nodes: vec![DagNodeDef {
                id: "a".into(),
                agent: "general".into(),
                task: "t".into(),
                depends_on: None,
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
            }],
            max_concurrency: None,
            fail_fast: None,
            direction: None,
        },
        None,
        Some("sess".into()),
    )
    .unwrap()
    .id;
    assert!(sink.count() >= 1, "plan must notify");

    // node completion fires (drive the fake launcher)
    engine.on_node_completed(
        &run_id,
        "a",
        NodeOutcome {
            success: true,
            error: None,
            duration_ms: 1,
            attempt: 1,
            total_attempts: 1,
            input_tokens: 10,
            output_tokens: 5,
            output: Some("done".into()),
        },
    );
    assert!(sink.count() >= 2, "node completion must notify");

    // run completion also fires (maybe_complete)
    let before = sink.count();
    engine.maybe_complete(&run_id);
    assert!(sink.count() > before, "run completion must notify");
}

/// Notify is safe even while the engine lock is held (persist sink lives
/// outside the lock) — no deadlock on nested state changes.
#[test]
fn notify_persist_safe_under_lock() {
    let engine = DagEngine::new();
    let sink = Arc::new(CountingSink::new());
    engine.set_persist_sink(Some(sink.clone() as Arc<dyn DagPersistSink>));
    // Directly exercise the internal helper while holding the lock would be
    // white-box; instead drive a real state change that internally holds the
    // lock while notifying (plan's insert scope).
    let run_id = engine
        .plan(
            DagRunDef {
                name: "p".into(),
                nodes: vec![DagNodeDef {
                    id: "a".into(),
                    agent: "general".into(),
                    task: "t".into(),
                    depends_on: None,
                    timeout: None,
                    cwd: None,
                    model: None,
                    thinking: None,
                    max_iterations: None,
                    tools: None,
                }],
                max_concurrency: None,
                fail_fast: None,
                direction: None,
            },
            None,
            None,
        )
        .unwrap()
        .id;
    assert!(!run_id.is_empty());
    assert!(sink.count() >= 1);
}
