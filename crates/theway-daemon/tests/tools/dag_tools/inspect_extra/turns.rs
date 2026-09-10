//! Per-turn summary rendering for `dag_inspect kind=transcript`
//! (mirrors the `turn_summary_text` surface in `src/tools/dag_tools/inspect.rs`).

use theway_core::multiagent::graph::types::{DagNodeDef, DagRunDef, NodeStatus};
use theway_core::multiagent::jobs::{
    JobTurnSummary, SubagentJob, SubagentJobInit, SubagentJobRegistry,
};

use super::super::{is_file_write_tool, tokens_short, transcript_text, turn_summary_text};

/// One recorded turn, `tools` in call order.
fn turn(index: u32, input_tokens: u64, output_tokens: u64, tools: &[&str]) -> JobTurnSummary {
    JobTurnSummary {
        index,
        input_tokens,
        output_tokens,
        tools: tools.iter().map(|name| name.to_string()).collect(),
    }
}

/// A registry job for `r1`/`n1` after `setup`, read back the way
/// `transcript_text` reads it (`job_for_node`).
fn seeded(setup: impl FnOnce(&mut SubagentJob)) -> (SubagentJobRegistry, SubagentJob) {
    let registry = SubagentJobRegistry::new();
    let job_id = registry.register(SubagentJobInit {
        agent: "planner".into(),
        source: "dag".into(),
        run_id: Some("r1".into()),
        node_id: Some("n1".into()),
        session_id: None,
    });
    registry.update(&job_id, setup);
    let job = registry.job_for_node("r1", "n1").unwrap();
    (registry, job)
}

/// The `kind=transcript` view of the seeded job's node at an explicit `tail`.
fn transcript_with(tail: usize, setup: impl FnOnce(&mut SubagentJob)) -> String {
    let (registry, _) = seeded(setup);
    let def = DagRunDef {
        name: "x".into(),
        nodes: vec![DagNodeDef {
            id: "n1".into(),
            agent: "planner".into(),
            task: "t".into(),
            depends_on: None,
            timeout: None,
            cwd: None,
            provider: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let mut run = theway_core::multiagent::graph::model::build_run(&def);
    run.node_mut("n1").unwrap().status = NodeStatus::Failed;
    transcript_text(run.node("n1").unwrap(), "r1", &registry, tail)
}

#[test]
fn turn_summary_text_reports_first_file_write_turn_and_tool_names() {
    // Arrange: twelve turns, only the last one writes.
    let (_, job) = seeded(|job| {
        let mut turns: Vec<JobTurnSummary> = Vec::new();
        for index in 1..=11u32 {
            turns.push(turn(index, 1200, 1000, &["read"]));
        }
        turns.push(turn(12, 60_200, 2400, &["write"]));
        job.turns = turns;
    });

    // Act
    let text = turn_summary_text(&job).expect("turns are recorded");

    // Assert: header counts turns and calls, and names the writing turn.
    assert!(
        text.contains("  per-turn: 12 turn(s) · 12 tool call(s) · first file write: turn 12"),
        "{text}"
    );
    // Rows carry the abbreviated token spend and the turn's tools in call order.
    assert!(text.contains("\n    t1    in 1.2k   out 1.0k   read"), "{text}");
    assert!(text.contains("\n    t12   in 60.2k  out 2.4k   write"), "{text}");
    // Aggregate line: call count descending.
    assert!(text.contains("\n  tools: read×11, write×1"), "{text}");
}

#[test]
fn turn_summary_text_reports_none_without_a_file_write() {
    // Arrange: read/bash only — a node that burned its budget before writing.
    let (_, job) = seeded(|job| {
        job.turns = vec![turn(1, 399_701, 800, &["read", "bash"]), turn(2, 500, 0, &[])];
    });

    // Act
    let text = turn_summary_text(&job).expect("turns are recorded");

    // Assert: no write tool anywhere in the recorded turns.
    assert!(text.contains("first file write: none"), "{text}");
    // A turn with no tool call is the "burned in thinking" signal.
    assert!(text.contains("\n    t2    in 500    out 0      (no tool call)"), "{text}");
    // Equal counts fall back to name order: bash before read.
    assert!(text.contains("\n  tools: bash×1, read×1"), "{text}");
    assert!(text.contains("\n    t1    in 399.7k out 800    read, bash"), "{text}");
}

#[test]
fn turn_summary_text_is_absent_without_recorded_turns() {
    // Arrange: a job from an older binary / a run whose counter never advanced.
    let (_, job) = seeded(|job| {
        job.messages.push(serde_json::json!({"role": "user", "content": "hello"}));
    });

    // Act + Assert
    assert!(turn_summary_text(&job).is_none());
}

#[test]
fn transcript_text_omits_turn_summary_without_turns() {
    // Arrange + Act
    let text = transcript_with(4_000, |job| {
        job.messages.push(serde_json::json!({"role": "user", "content": "hello"}));
    });

    // Assert: the existing parts stay, the summary part is missing.
    assert!(text.contains("messages: 1"), "{text}");
    assert!(!text.contains("per-turn:"), "{text}");
}

#[test]
fn turn_summary_text_marks_dropped_oldest_turns() {
    // Arrange: a retained window plus the truncation flag.
    let (_, job) = seeded(|job| {
        job.turns = vec![turn(80, 1000, 500, &["read"])];
        job.turns_truncated = true;
    });

    // Act
    let text = turn_summary_text(&job).expect("turns are recorded");

    // Assert: the note follows the aggregate line.
    assert!(text.contains("per-turn: 1 turn(s)"), "{text}");
    let tools_line = text.find("  tools: ").expect("aggregate line");
    let note = text.find("  (turns 已截断, 最旧的轮次已丢弃)").expect("note");
    assert!(note > tools_line, "{text}");
}

#[test]
fn transcript_text_keeps_turn_summary_after_the_truncated_body() {
    // Arrange: a transcript far longer than the requested tail.
    let text = transcript_with(40, |job| {
        job.turns = vec![turn(1, 1200, 800, &["read", "write"])];
        job.messages = (0..60)
            .map(|i| serde_json::json!({"role": "user", "content": format!("padding message {i}")}))
            .collect();
    });

    // Assert: the body was cut …
    let cut = text.find("字符, 截断)").expect("truncated body");
    // … and the summary still follows the cut.
    let summary = text.find("per-turn:").expect("turn summary");
    assert!(summary > cut, "{text}");
    assert!(text.contains("first file write: turn 1"), "{text}");
    assert!(text.contains("\n  tools: read×1, write×1"), "{text}");
}

#[test]
fn tokens_short_abbreviates_from_one_thousand() {
    assert_eq!(tokens_short(0), "0");
    assert_eq!(tokens_short(999), "999");
    assert_eq!(tokens_short(1000), "1.0k");
    assert_eq!(tokens_short(1200), "1.2k");
    assert_eq!(tokens_short(60_200), "60.2k");
    assert_eq!(tokens_short(399_701), "399.7k");
}

#[test]
fn is_file_write_tool_matches_only_write_and_edit() {
    // `bash` writes files too, but only these two tools name the operation.
    assert!(is_file_write_tool("write"));
    assert!(is_file_write_tool("edit"));
    assert!(!is_file_write_tool("bash"));
    assert!(!is_file_write_tool("read"));
}
