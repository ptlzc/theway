//! Node launch lifecycle: resolution, completion, overrides, timeout, cancel.

use super::*;

#[tokio::test]
async fn unknown_agent_fails_node_synchronously() {
    let engine = engine_with_launcher(faux_model(), faux_stream("nope"));
    // plan → tick → launch all happen synchronously; the unknown-agent path never
    // spawns a task, so the run is already Failed when plan returns.
    let run_id = plan_single_node(&engine, "no-such-agent", "hello", None);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    assert_eq!(
        node.error.as_deref(),
        Some("unknown agent \"no-such-agent\"")
    );
    assert_eq!(node.input_tokens, Some(0));
}

#[tokio::test]
async fn missing_model_fails_the_node_with_clear_error() {
    // Model is session-level (injected by the client): a launcher created with
    // no model must fail any node it spawns with a clear, retryable error.
    let engine = engine_with_launcher_model(None, faux_stream("nope"));
    let run_id = plan_single_node(&engine, "general", "do the thing", None);
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    assert_eq!(
        node.error.as_deref(),
        Some(
            "no model set for this session; select a model in the TUI before launching DAG nodes (or set provider + model on the node)"
        )
    );
    assert!(node.input_tokens.is_none() || node.input_tokens == Some(0));
}

#[tokio::test]
async fn known_agent_completes_with_output_and_tokens() {
    let engine = engine_with_launcher(faux_model(), faux_stream("dag done"));
    let run_id = plan_single_node(&engine, "general", "do the thing", None);
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Succeeded);
    assert_eq!(node.error, None);
    assert_eq!(node.output.as_deref(), Some("dag done"));
    assert_eq!(node.input_tokens, Some(0));
    assert_eq!(node.output_tokens, Some(0));
    assert!(node.result.as_ref().unwrap().total_attempts >= 1);
}

#[tokio::test]
async fn model_override_rewrites_id_and_still_completes() {
    let engine = engine_with_launcher(faux_model(), faux_stream("ok"));
    let def = DagRunDef {
        name: "override".into(),
        nodes: vec![DagNodeDef {
            id: "a".into(),
            agent: "general".into(),
            task: "t".into(),
            depends_on: None,
            timeout: None,
            cwd: None,
            provider: None,
            model: Some("other-model".into()),
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let run_id = engine.plan(def, None, None).unwrap().id;
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(results, vec![(run_id.clone(), false)]);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Succeeded);
}

#[tokio::test]
async fn node_timeout_fails_the_node() {
    let engine = engine_with_launcher(faux_model(), stalled_stream());
    let run_id = plan_single_node(&engine, "general", "hang", Some(1));
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    let err = node.error.as_deref().unwrap();
    assert!(err.contains("no output for 1s (idle timeout)"), "{err}");
}

/// Idle timeout must NOT be a wall-clock cap: a node that keeps emitting
/// activity (token deltas) past the idle window survives to completion.
#[tokio::test]
async fn idle_timeout_reschedules_on_activity() {
    let engine = engine_with_launcher(faux_model(), slow_stream());
    let run_id = plan_single_node(&engine, "general", "stream", Some(1));
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(
        results,
        vec![(run_id.clone(), false)],
        "activity must keep the run alive"
    );
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Succeeded);
    assert_eq!(node.output.as_deref(), Some("slow done"));
}

#[tokio::test]
async fn run_cancel_aborts_the_node_job() {
    let engine = engine_with_launcher(faux_model(), stalled_stream());
    let run_id = plan_single_node(&engine, "general", "hang", None);
    // Let the node reach Running (launch is a spawned task) before cancelling.
    tokio::time::sleep(Duration::from_millis(100)).await;
    engine.cancel_run(&run_id, Some("test cancel"));
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Cancelled);
    // Give the aborted job time to unwind; a stale completion report must not flip
    // the cancelled state.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Cancelled);
    assert_eq!(run.error.as_deref(), Some("test cancel"));
}

#[test]
fn cap_chars_truncates_on_char_boundary() {
    assert_eq!(cap_chars("short", 10), "short");
    let long = "x".repeat(100);
    assert_eq!(cap_chars(&long, 16).chars().count(), 16);
}

#[test]
fn launch_ignores_missing_run_or_node() {
    let engine = Arc::new(DagEngine::new());
    let launcher = node_launcher(
        engine.clone(),
        Some(faux_model()),
        Some(faux_stream("unused")),
        PathBuf::from("."),
        crate::multiagent::jobs::SubagentJobRegistry::new(),
        Arc::new(|_| Vec::new()),
        test_launch_resolver(),
    );

    // Neither exists → no panic, no task spawned.
    NodeLauncher::launch(
        &*launcher,
        "no-such-run",
        "no-such-node",
        CancellationToken::new(),
    );

    // Run exists but node does not → same silent return.
    let run_id = plan_single_node(&engine, "general", "hello", None);
    NodeLauncher::launch(&*launcher, &run_id, "missing-node", CancellationToken::new());
}

#[tokio::test]
async fn model_override_same_id_uses_parent_model() {
    let engine = engine_with_launcher(faux_model(), faux_stream("ok"));
    let def = DagRunDef {
        name: "same-model".into(),
        nodes: vec![DagNodeDef {
            id: "a".into(),
            agent: "general".into(),
            task: "t".into(),
            depends_on: None,
            timeout: None,
            cwd: None,
            provider: None,
            model: Some("faux".into()),
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    let run_id = engine.plan(def, None, None).unwrap().id;
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;
    assert_eq!(results, vec![(run_id.clone(), false)]);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    assert_eq!(run.node("a").unwrap().status, NodeStatus::Succeeded);
}

fn catalog_model(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        name: format!("{provider} {id}"),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from(provider),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

/// The regression case behind the feature request: an explicit
/// `provider + model` pair must launch even when the parent session has no
/// model (e.g. a collapse-inherited session with empty model state).
#[tokio::test]
async fn explicit_provider_model_resolves_without_parent_model() {
    let provider = "test-node-provider";
    let id = "test-node-model";
    theway_llm_provider::register_custom_model(catalog_model(provider, id));

    let engine = engine_with_launcher_model(None, faux_stream("catalog ok"));
    let mut node = node_def("general");
    node.provider = Some(provider.into());
    node.model = Some(id.into());
    let run_id = plan_node(&engine, node);
    let results = engine
        .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
        .await;

    theway_llm_provider::unregister_custom_model(
        &theway_llm_provider::Provider::from(provider),
        id,
    );

    assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Completed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Succeeded);
    assert_eq!(node.output.as_deref(), Some("catalog ok"));
}

#[tokio::test]
async fn unknown_provider_model_pair_fails_node_synchronously() {
    let engine = engine_with_launcher(faux_model(), faux_stream("unreachable"));
    let mut node = node_def("general");
    node.provider = Some("test-no-such-provider".into());
    node.model = Some("x".into());
    let run_id = plan_node(&engine, node);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    let err = node.error.as_deref().unwrap();
    assert!(err.contains("model provider not found in catalog"), "{err}");
    assert!(err.contains("test-no-such-provider"), "{err}");
}

#[tokio::test]
async fn provider_without_model_fails_node_synchronously() {
    let engine = engine_with_launcher(faux_model(), faux_stream("unreachable"));
    let mut node = node_def("general");
    node.provider = Some("test-node-provider".into());
    let run_id = plan_node(&engine, node);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    let err = node.error.as_deref().unwrap();
    assert!(
        err.contains("provider override requires a model override"),
        "{err}"
    );
}

#[tokio::test]
async fn invalid_thinking_fails_node_synchronously() {
    let engine = engine_with_launcher(faux_model(), faux_stream("unreachable"));
    let mut node = node_def("general");
    node.thinking = Some("ultra".into());
    let run_id = plan_node(&engine, node);
    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Failed);
    let node = run.node("a").unwrap();
    assert_eq!(node.status, NodeStatus::Failed);
    let err = node.error.as_deref().unwrap();
    assert!(
        err.contains("invalid thinking level: ultra"),
        "{err}"
    );
}
