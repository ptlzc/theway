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
use theway_core::harness::graph_engineering::engine::DagEngine;
use theway_core::{AgentTool, AgentToolResult, StreamFn};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use tokio_util::sync::CancellationToken;

// Tool modules under test, pulled in exactly like `task_tool_e2e` pulls `task.rs`.
#[path = "../src/tools/dag_tools.rs"]
mod dag_tools;
#[path = "../src/tools/node_launcher.rs"]
mod node_launcher;
#[path = "../src/tools/subagent_runner.rs"]
mod subagent_runner;
#[path = "../src/tools/subagent_specs.rs"]
mod subagent_specs;

// ── shims for the spec tool-set factories (never executed by this e2e) ─────
// `subagent_specs::*_tools()` closures build concrete tools via `super::` and
// `crate::config::memory_dir`; provide inert stand-ins so the included modules
// compile inside this test crate. The agent loop stops after the faux model's
// single turn, so no stub tool is ever dispatched.
mod config {
    pub fn memory_dir() -> std::path::PathBuf {
        std::env::temp_dir()
    }
}

fn default_tools(_dir: std::path::PathBuf) -> Vec<Arc<dyn AgentTool>> {
    vec![Arc::new(read::ReadTool), Arc::new(bash::BashTool)]
}

fn subagent_read_only_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(git::GitTool),
    ]
}

macro_rules! stub_tool {
    ($mod:ident, $ty:ident, $label:literal) => {
        pub mod $mod {
            use async_trait::async_trait;
            use serde_json::{Value, json};
            use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
            use tokio_util::sync::CancellationToken;

            pub struct $ty;
            #[async_trait]
            impl AgentTool for $ty {
                fn definition(&self) -> &theway_llm_provider::Tool {
                    static DEF: std::sync::OnceLock<theway_llm_provider::Tool> =
                        std::sync::OnceLock::new();
                    DEF.get_or_init(|| theway_llm_provider::Tool {
                        name: stringify!($ty).into(),
                        description: "e2e stub (spec tool-set factory never runs)".into(),
                        parameters: json!({}),
                    })
                }
                fn label(&self) -> &str {
                    $label
                }
                async fn execute(
                    &self,
                    _id: &str,
                    _params: Value,
                    _cancel: CancellationToken,
                    _on_update: Option<AgentToolUpdate>,
                ) -> Result<AgentToolResult, AgentToolError> {
                    Err(AgentToolError::Message("e2e stub tool executed".into()))
                }
            }
        }
    };
}

stub_tool!(read, ReadTool, "read");
stub_tool!(ls, LsTool, "ls");
stub_tool!(grep, GrepTool, "grep");
stub_tool!(find, FindTool, "find");
stub_tool!(bash, BashTool, "bash");
stub_tool!(git, GitTool, "git");
stub_tool!(web_fetch, WebFetchTool, "web_fetch");
stub_tool!(web_search, WebSearchTool, "web_search");

impl web_search::WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

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
        theway_core::harness::subagents::registry::SubagentJobRegistry::new(),
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
