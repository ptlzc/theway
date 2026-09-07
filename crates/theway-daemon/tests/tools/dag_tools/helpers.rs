use super::super::utils::{
    current_time_ms, foreign_session, mine, node_def_from_json, resolve_dag,
};
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
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
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
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
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

#[test]
fn node_result_text_running_node_includes_live_preview() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
    let node = run.node_mut("a").unwrap();
    node.status = NodeStatus::Running;
    node.started_at = Some(0);
    node.last_active_at = Some(current_time_ms());
    node.live_preview = Some("live tail".into());
    let text = node_result_text(run.node("a").unwrap(), 800);
    assert!(text.contains("a [x] — running"), "{text}");
    assert!(text.contains("  last-active: "), "{text}");
    assert!(text.contains("  live preview (实时输出, tail 800):\nlive tail"), "{text}");
}

#[test]
fn status_counts_ready_segment() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a"), node_def("b")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
    run.node_mut("a").unwrap().status = NodeStatus::Succeeded;
    run.node_mut("b").unwrap().status = NodeStatus::Ready;
    assert_eq!(status_counts(&run), "done 1/2 · ready 1");
}

#[test]
fn mine_and_foreign_session_matrix() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
    run.id = "dag-1".into();

    run.session_id = None;
    assert!(mine(&run, &None));
    assert!(mine(&run, &Some("s".into())));
    run.session_id = Some("s".into());
    assert!(mine(&run, &None));
    assert!(mine(&run, &Some("s".into())));
    assert!(!mine(&run, &Some("other".into())));
    assert_eq!(foreign_session(&run, &Some("other".into())), Some("dag-1 属于其他会话 (s…), 当前会话是 other…。多 agent 会话的 DAG 相互隔离, 只可操作本会话创建的 DAG。".into()));
    assert_eq!(foreign_session(&run, &None), None);
    assert_eq!(foreign_session(&run, &Some("s".into())), None);
}

#[test]
fn resolve_dag_empty_engine_reports_no_dags() {
    let engine = DagEngine::new();
    assert_eq!(
        resolve_dag(&engine, &None, None),
        Err("当前没有 DAG。先用 dag_plan 定义一个。".to_string())
    );
}

#[test]
fn resolve_dag_unknown_explicit_dag_returns_hint() {
    let engine = DagEngine::new();
    let err = resolve_dag(&engine, &None, Some("nope")).unwrap_err();
    assert!(err.contains("未知 DAG: nope"), "got: {err}");
}

#[test]
fn node_result_text_minimal_node_omits_optional_sections() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
    let node = run.node_mut("a").unwrap();
    node.status = NodeStatus::Pending;
    let text = node_result_text(run.node("a").unwrap(), 800);
    assert!(text.contains("a [x] — pending"), "{text}");
    assert!(text.contains("  task: task a"), "{text}");
    assert!(!text.contains("started:"), "{text}");
    assert!(!text.contains("tokens:"), "{text}");
    assert!(!text.contains("error:"), "{text}");
    assert!(!text.contains("output (tail"), "{text}");
    assert!(!text.contains("live preview"), "{text}");
}

#[test]
fn node_result_text_running_without_last_active_or_live_preview_omits_both() {
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![node_def("a")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
    let node = run.node_mut("a").unwrap();
    node.status = NodeStatus::Running;
    let text = node_result_text(run.node("a").unwrap(), 800);
    assert!(text.contains("a [x] — running"), "{text}");
    assert!(!text.contains("last-active:"), "{text}");
    assert!(!text.contains("live preview"), "{text}");
}

fn node_def(id: &str) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "x".to_string(),
        task: format!("task {id}"),
        depends_on: None,
        timeout: None,
        cwd: None,
        provider: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

// ── node_def_from_json: provider / model / thinking / maxIterations / tools ──

#[test]
fn node_def_from_json_parses_provider_model_thinking_budget_and_tools() {
    let def = node_def_from_json(&json!({
        "id": "a",
        "agent": "explorer",
        "task": "read",
        "provider": "deepseek",
        "model": "deepseek-v4-flash",
        "thinking": "high",
        "maxIterations": 8,
        "tools": ["read", "bash"],
    }));
    assert_eq!(def.id, "a");
    assert_eq!(def.provider, Some("deepseek".to_string()));
    assert_eq!(def.model, Some("deepseek-v4-flash".to_string()));
    assert_eq!(def.thinking, Some("high".to_string()));
    assert_eq!(def.max_iterations, Some(8));
    assert_eq!(
        def.tools,
        Some(vec!["read".to_string(), "bash".to_string()])
    );
}

#[test]
fn node_def_from_json_missing_overrides_stay_none() {
    let def = node_def_from_json(&json!({
        "id": "a",
        "agent": "explorer",
        "task": "read",
    }));
    assert_eq!(def.provider, None);
    assert_eq!(def.model, None);
    assert_eq!(def.thinking, None);
    assert_eq!(def.max_iterations, None);
    assert_eq!(def.tools, None);
}

#[test]
fn node_def_from_json_wrong_types_fall_back_to_none() {
    let def = node_def_from_json(&json!({
        "id": "a",
        "agent": "explorer",
        "task": "read",
        "provider": 7,
        "model": 8,
        "thinking": 9,
        "maxIterations": "eight",
        "tools": "read",
    }));
    assert_eq!(def.provider, None);
    assert_eq!(def.model, None);
    assert_eq!(def.thinking, None);
    assert_eq!(def.max_iterations, None);
    assert_eq!(def.tools, None);
    // Non-string entries inside the tools array are dropped, not an error.
    let def = node_def_from_json(&json!({
        "id": "a",
        "agent": "explorer",
        "task": "read",
        "tools": ["read", 7, null],
    }));
    assert_eq!(def.tools, Some(vec!["read".to_string()]));
}
