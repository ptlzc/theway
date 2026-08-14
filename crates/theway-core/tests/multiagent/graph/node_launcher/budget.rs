//! Per-node iteration-budget override and tool allowlist (launch-time).

use super::*;

/// `node.max_iterations` wins over the spec default (16 in the test resolver):
/// the looping stream only stops at the budget, and the failure message carries
/// the node-level cap.
#[tokio::test]
async fn max_iterations_override_caps_agent_loop() {
    let engine = engine_with_launcher(faux_model(), looping_stream());
    let mut node = node_def("general");
    node.max_iterations = Some(2);
    let run_id = plan_node(&engine, node);
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    assert_eq!(node.error.as_deref(), Some("max iterations (2) exceeded"));
}

/// An unknown allowlist name fails the node synchronously — same shape as the
/// unknown-agent path: no job spawned, no panic, reason visible to the
/// orchestrator.
#[tokio::test]
async fn tools_allowlist_unknown_name_fails_node_synchronously() {
    let engine = engine_with_tools(
        faux_model(),
        faux_stream("unreachable"),
        vec![RecordingTool::arc("bash")],
    );
    let mut node = node_def("general");
    node.tools = Some(vec!["nope".into()]);
    // plan → tick → launch run synchronously; the filter failure is reported
    // before plan returns, so no wait is needed.
    let run_id = plan_node(&engine, node);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    let err = node.error.as_deref().unwrap();
    assert!(err.contains("unknown tool in allowlist: nope"), "{err}");
    assert!(err.contains("available: bash"), "{err}");
}

/// A valid allowlist narrows the sub-harness tool set: with `bash` allowed the
/// streamed tool call executes; with only `read` allowed `bash` is unreachable
/// (the harness reports "No tool registered") while the run still completes.
#[tokio::test]
async fn tools_allowlist_narrows_the_tool_set() {
    for (allow, expect_bash) in [(["bash"], true), (["read"], false)] {
        let bash = RecordingTool::arc("bash");
        let read = RecordingTool::arc("read");
        let engine = engine_with_tools(
            faux_model(),
            tool_call_then_done("bash", "done via bash"),
            vec![bash.clone(), read.clone()],
        );
        let mut node = node_def("general");
        node.tools = Some(allow.map(String::from).to_vec());
        let run_id = plan_node(&engine, node);
        let results = engine
            .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
            .await;
        assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Completed);
        let node = run.node("a").unwrap();
        assert_eq!(node.status, NodeStatus::Succeeded);
        assert_eq!(node.output.as_deref(), Some("done via bash"));
        assert_eq!(bash.was_called(), expect_bash, "allowlist: {allow:?}");
        assert!(!read.was_called(), "the stream never calls `read`");
    }
}
