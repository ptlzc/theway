//! End-to-end test for the DAG tool face over the REAL node launcher (p3c-wire wiring):
//! `dag_plan` a 2-node DAG (A→B) → `dag_wait` harvests both nodes → `dag_status` reports
//! `done 2/2`.
//!
//! Mirrors `task_tool_e2e.rs`: drives the tools directly with a faux model + faux StreamFn,
//! so the full path — tool → engine → `NodeLauncherImpl` → one real `AgentHarness` per node
//! → node completion → harvest — runs deterministically with no provider key. The spec
//! tool-set factories are never executed (the faux model stops after one turn), so the
//! concrete tool shims below are inert stand-ins that only need to compile.

use std::sync::Arc;

use serde_json::json;
use theway_core::runtime::graph_engineering::engine::DagEngine;
use theway_core::{AgentTool, AgentToolResult, StreamFn};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use tokio_util::sync::CancellationToken;

// Tool modules under test, pulled in exactly like `task_tool_e2e` pulls `task.rs`.
// (Bodies moved into theway-core by openspec tools-into-core; assembly stayed server-side.)
// e2e includes engine/src files by `#[path]`; those files may contain a
// `tests_bridge!("...")` call (module tests live in `tests/<mirror>/`, see
// docs/RUST_TEST_FILES.md). This test crate is a separate binary, so the macro
// from lib.rs is not in scope — define it here (before the includes).
#[cfg(test)]
macro_rules! tests_bridge {
    ($path:literal) => {
        #[path = $path]
        mod tests;
    };
}

#[path = "../../core/src/tools/dag_tools.rs"]
mod dag_tools;
#[path = "../../core/src/tools/node_launcher.rs"]
mod node_launcher;
#[path = "../../core/src/tools/subagent_runner.rs"]
mod subagent_runner;
#[path = "../../core/src/tools/subagent_specs.rs"]
mod subagent_specs;

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
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

fn faux_stream(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

fn tool_by<'a>(tools: &'a [Arc<dyn AgentTool>], name: &str) -> &'a dyn AgentTool {
    tools
        .iter()
        .find(|t| t.label() == name)
        .unwrap_or_else(|| panic!("missing tool {name}"))
        .as_ref()
}

fn text_of(res: AgentToolResult) -> String {
    match &res.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

#[tokio::test]
async fn dag_plan_wait_status_completes_2_node_dag_with_real_launcher() {
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(node_launcher::node_launcher(
        engine.clone(),
        faux_model(),
        Some(faux_stream("node result")),
        std::env::temp_dir(),
        theway_core::runtime::subagents::registry::SubagentJobRegistry::new(),
        // Tool-set resolver: subagents never call tools in this e2e (the faux model
        // stops after one turn), so an empty tool set per spec suffices.
        Arc::new(|_| Vec::new()),
    )));
    let tools = dag_tools::DagTools::new(engine.clone(), Some("e2e-session".to_string()));

    // dag_plan: 2-node mermaid DAG (A → B), both nodes are `explorer` subagents.
    let plan = tool_by(&tools, "dag_plan")
        .execute(
            "e2e",
            json!({
                "name": "e2e",
                "mermaid": "graph TD\nA[\"explorer: node A\"] --> B[\"explorer: node B\"]",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let plan_text = text_of(plan);
    assert!(
        plan_text.contains("✓ 已创建并自动启动 dag-1"),
        "plan should auto-start dag-1: {plan_text}"
    );

    // dag_wait: harvests both nodes through the real launcher's AgentHarness runs.
    let run_id = engine.list_runs()[0].id.clone();
    let wait = tool_by(&tools, "dag_wait")
        .execute(
            "e2e",
            json!({ "dagId": run_id, "timeout": 60 }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let wait_text = text_of(wait);
    assert!(
        wait_text.contains("dag-1 已完成: done 2/2"),
        "dag_wait should harvest 2/2 done: {wait_text}"
    );

    // dag_status: summary reflects the completed run.
    let status = tool_by(&tools, "dag_status")
        .execute(
            "e2e",
            json!({ "dagId": run_id }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let status_text = text_of(status);
    assert!(
        status_text.contains("done 2/2"),
        "dag_status should show done 2/2: {status_text}"
    );
}
