//! Tests for `multiagent::graph::model::mermaid` — split out of src
//! (see docs/rust-test-files.md). The mirror directory is `mermaid_extra`
//! because `tests/multiagent/graph/model/mermaid.rs` already exists for the
//! model module's public-facing test suite.

use std::collections::HashMap;

use super::*;
use crate::multiagent::graph::model::build_run;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef, Direction, NodeStatus};

fn node_def(id: &str, deps: &[&str]) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "x".to_string(),
        task: "task".to_string(),
        depends_on: if deps.is_empty() {
            None
        } else {
            Some(deps.iter().map(|s| s.to_string()).collect())
        },
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(direction: Direction) -> DagRunDef {
    DagRunDef {
        name: "t".into(),
        nodes: vec![node_def("a", &[])],
        max_concurrency: None,
        fail_fast: None,
        direction: Some(direction),
    }
}

#[test]
fn split_ampersand_respects_quotes_and_escapes() {
    assert_eq!(
        split_ampersand_outside_quotes("A & B"),
        vec!["A ", " B"]
    );
    assert_eq!(
        split_ampersand_outside_quotes("A[\"x & y\"] & B"),
        vec!["A[\"x & y\"] ", " B"]
    );
    assert_eq!(
        split_ampersand_outside_quotes("A['x & y']"),
        vec!["A['x & y']"]
    );
}

#[test]
fn map_id_rewrites_hyphens_and_deduplicates_collisions() {
    let mut id_map = HashMap::new();
    let mut reverse = HashMap::new();

    assert_eq!(map_id("a-b", &mut id_map, &mut reverse), "a_b");
    assert_eq!(map_id("a_b", &mut id_map, &mut reverse), "a_b_");
    assert_eq!(map_id("a-b", &mut id_map, &mut reverse), "a_b");
}

#[test]
fn preprocess_blank_and_comment_only_yields_empty_declared() {
    let prep = preprocess("%% header\n%% tail");
    assert!(prep.declared.is_empty());
    assert!(prep.errors.is_empty());
    assert_eq!(prep.direction, Direction::Td);
    assert!(prep.normalized.is_empty());
}

#[test]
fn parse_mermaid_unlabeled_node_reports_missing_agent_and_task() {
    let res = parse_mermaid("graph TD\nA");
    assert!(res.errors.iter().any(|e| e.contains("label 需以")));
    assert!(res.errors.iter().any(|e| e.contains("缺少 task")));
}

#[test]
fn render_mermaid_lr_uses_lr_and_skips_pending_class() {
    let mut run = build_run(&run_def(Direction::Lr));
    run.node_mut("a").unwrap().status = NodeStatus::Succeeded;
    let rendered = render_mermaid(&run);
    assert!(rendered.starts_with("graph LR"));
    assert!(rendered.contains("classDef succeeded"));
    assert!(rendered.contains("class a succeeded"));
}

#[test]
fn node_short_label_contains_status_deps_id_agent_and_task() {
    let mut run = build_run(&DagRunDef {
        name: "t".into(),
        nodes: vec![node_def("b", &["a"])],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    });
    run.node_mut("b").unwrap().status = NodeStatus::Running;
    let label = node_short_label(run.node("b").unwrap());
    assert!(label.starts_with("[run] [a] b [x] task"));
}

#[test]
fn thousands_formats_with_commas() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(12), "12");
    assert_eq!(thousands(123), "123");
    assert_eq!(thousands(1234), "1,234");
    assert_eq!(thousands(1234567), "1,234,567");
}

#[test]
fn run_summary_line_running_without_tokens_has_no_token_part() {
    let mut run = build_run(&run_def(Direction::Td));
    run.id = "dag-1".into();
    let line = run_summary_line(&run);
    assert!(line.contains("dag-1"));
    assert!(!line.contains("↑"));
    assert!(!line.contains("tok/s"));
    assert!(!line.contains("[completed]"));
}

#[test]
fn first_line_trims_and_truncates() {
    assert_eq!(first_line("  hello\nworld", 10), "hello");
    assert_eq!(first_line("", 10), "");
    assert_eq!(first_line("abcdef", 3), "abc…");
}

#[test]
fn render_tree_survives_dependency_cycle() {
    let mut run = build_run(&DagRunDef {
        name: "t".into(),
        nodes: vec![node_def("a", &["b"]), node_def("b", &["a"])],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    });
    run.id = "dag-cycle".into();

    let tree = render_tree(&run);

    assert!(tree.contains("[wait]"));
    assert!(tree.lines().count() >= 2);
}
