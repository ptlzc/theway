//! Tests for `multiagent::graph::engine::helpers` — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::multiagent::graph::model::build_run;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef, NodeResult};

fn run_def() -> DagRunDef {
    DagRunDef {
        name: "t".to_string(),
        nodes: vec![DagNodeDef {
            id: "a".to_string(),
            agent: "x".to_string(),
            task: "task".to_string(),
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
    }
}

#[test]
fn emit_state_stamps_last_activity_at() {
    // Arrange
    let mut run = build_run(&run_def());
    run.last_activity_at = 0;

    // Act
    emit_state(&mut run);

    // Assert
    assert!(run.last_activity_at > 0);
}

#[test]
fn reset_node_reverts_all_runtime_fields_to_pending() {
    // Arrange
    let mut run = build_run(&run_def());
    let node = run.node_mut("a").unwrap();
    node.status = NodeStatus::Failed;
    node.started_at = Some(1);
    node.completed_at = Some(2);
    node.error = Some("boom".into());
    node.job_id = Some("job-1".into());
    node.result = Some(NodeResult {
        success: false,
        error: Some("boom".into()),
        duration_ms: Some(10),
        attempt: 2,
        total_attempts: 2,
    });
    node.attempt = 3;
    node.input_tokens = Some(4);
    node.output_tokens = Some(5);

    // Act
    reset_node(node);

    // Assert
    assert_eq!(node.status, NodeStatus::Pending);
    assert_eq!(node.started_at, None);
    assert_eq!(node.completed_at, None);
    assert_eq!(node.error, None);
    assert_eq!(node.job_id, None);
    assert_eq!(node.result, None);
    assert_eq!(node.attempt, 0);
    assert_eq!(node.input_tokens, None);
    assert_eq!(node.output_tokens, None);
}

#[test]
fn push_unique_adds_new_and_skips_duplicate() {
    // Arrange
    let mut vec = vec!["a".to_string()];

    // Act
    push_unique(&mut vec, "a");
    push_unique(&mut vec, "b");

    // Assert
    assert_eq!(vec, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn run_counter_parses_dag_suffix_or_zero() {
    assert_eq!(run_counter("dag-12"), 12);
    assert_eq!(run_counter("dag-0"), 0);
    assert_eq!(run_counter("goal-3"), 0);
    assert_eq!(run_counter("dag-abc"), 0);
    assert_eq!(run_counter("dag-12x"), 0);
}

#[test]
fn cap_chars_truncates_on_char_boundary() {
    assert_eq!(cap_chars("abcdef", 10), "abcdef");
    assert_eq!(cap_chars("abcdef", 3), "abc");
    assert_eq!(cap_chars("aé日b", 2), "aé");
}

#[test]
fn panic_message_downcasts_str_string_and_other() {
    let s = "boom";
    let payload = Box::new(s) as Box<dyn std::any::Any + Send>;
    assert_eq!(panic_message(&payload), "boom");

    let payload = Box::new("boom".to_string()) as Box<dyn std::any::Any + Send>;
    assert_eq!(panic_message(&payload), "boom");

    let payload = Box::new(42u32) as Box<dyn std::any::Any + Send>;
    assert_eq!(panic_message(&payload), "launcher panicked");
}
