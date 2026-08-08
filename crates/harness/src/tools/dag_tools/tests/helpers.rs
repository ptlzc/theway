use super::super::*;
use super::*;

// ── helpers ──────────────────────────────────────────────────────────────

#[test]
fn iso_time_and_civil_days() {
    assert_eq!(iso_time_ms(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso_time_ms(1_735_689_600_000), "2025-01-01T00:00:00.000Z");
    assert_eq!(iso_time_ms(1_735_689_601_234), "2025-01-01T00:00:01.234Z");
    assert_eq!(iso_time_ms(-1), "1969-12-31T23:59:59.999Z");
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(20_089), (2025, 1, 1));
}

#[test]
fn thousands_and_tail_truncate() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(12_345), "12,345");
    assert_eq!(thousands(1_234_567), "1,234,567");
    assert_eq!(tail_truncate("hello", 800), "hello");
    let long = "x".repeat(100);
    let t = tail_truncate(&long, 10);
    assert!(t.starts_with("…(100 字符, 截断)"), "{t}");
    assert!(t.ends_with(&"x".repeat(10)), "{t}");
}

#[test]
fn status_counts_segments() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a"), node_def("b"), node_def("c"), node_def("d")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::harness::graph_engineering::graph::build_run(&def);
    run.id = "dag-1".into();
    run.node_mut("a").unwrap().status = NodeStatus::Succeeded;
    run.node_mut("b").unwrap().status = NodeStatus::Running;
    run.node_mut("c").unwrap().status = NodeStatus::Cancelled;
    run.node_mut("d").unwrap().status = NodeStatus::Failed;
    assert_eq!(status_counts(&run), "done 1/4 · run 1 · cancel 1 · fail 1");
    run.node_mut("b").unwrap().status = NodeStatus::Skipped;
    assert_eq!(status_counts(&run), "done 2/4 · cancel 1 · fail 1");
}

#[test]
fn node_result_text_pieces() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::harness::graph_engineering::graph::build_run(&def);
    let node = run.node_mut("a").unwrap();
    node.status = NodeStatus::Succeeded;
    node.started_at = Some(0);
    node.completed_at = Some(1_500);
    node.input_tokens = Some(12_000);
    node.output_tokens = Some(456);
    node.error = Some("boom".into());
    node.output = Some("out".into());
    let text = node_result_text(run.node("a").unwrap(), 800);
    assert!(text.contains("a [x] — succeeded"), "{text}");
    assert!(
        text.contains("  started: 1970-01-01T00:00:00.000Z"),
        "{text}"
    );
    assert!(text.contains("  duration: 1.5s"), "{text}");
    assert!(text.contains("  tokens: ↑12,000 ↓456"), "{text}");
    assert!(text.contains("  error: boom"), "{text}");
    assert!(text.contains("  output (tail 800):\nout"), "{text}");
    // node_summary_line comes from graph.rs — sanity that it stays in sync.
    assert!(node_summary_line(run.node("a").unwrap()).starts_with("[done] a [x]"));
}

fn node_def(id: &str) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "x".to_string(),
        task: format!("task {id}"),
        depends_on: None,
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
    }
}
