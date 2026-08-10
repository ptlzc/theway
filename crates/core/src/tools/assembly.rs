//! `assembly` — engine-side tool assembly for the harness-runtime tools.
//!
//! Mirrors the app-layer assembly in the `theway` server crate, but for the tools the
//! ENGINE itself owns: DAG orchestration (`dag_*`), the `subagent` delegation tool, the
//! skill family (skill / install / builder / state / remove), and memory. These are
//! runtime capabilities — they do not depend on the execution environment — so the
//! engine knows how to wire them. The app layer only supplies its local-execution tools
//! (bash / fs / git / grep / web) as a factory.
//!
//! Tool-set policy:
//! - Main agent: [`engine_tools`] (everything the engine owns: dag_*, subagent, skills,
//!   memory) + the app's local tools.
//! - Subagents (both the `subagent` tool and DAG nodes): [`subagent_tools`] — ONE
//!   uniform set for every spec = engine tools MINUS the two orchestration tools
//!   (`subagent` recursion and `dag_*` are not for subagents) + the app's local tools.
//!   Per-spec differences are NOT enforced at the tool level anymore; the spec's system
//!   prompt and the parent's task prompt define behavior.
//!
//! The app-layer injection point is a local-tools factory ([`LocalToolsFn`]) and the
//! skill harness cell; everything else the engine wires itself.

use std::path::Path;
use std::sync::Arc;

use theway_llm_provider::Model;

use crate::runtime::graph_engineering::engine::DagEngine;
use crate::runtime::subagents::node_launcher::ToolSetResolver;
use crate::runtime::subagents::registry::SubagentJobRegistry;
use crate::{AgentTool, StreamFn};

use super::dag_tools;
use super::install_skill;
use super::memory::MemoryTool;
use super::remove_skill;
use super::set_skill_state;
use super::skill::{self, SkillHarnessCell};
use super::skill_builder;
use super::subagent::{SubagentTool, SubagentToolsFn};
use crate::runtime::subagents::launch::LaunchResolver;

/// App-layer factory producing the LOCAL execution tools (bash / fs / git / web / …) —
/// the part the engine cannot know about. Injected once at assembly; every harness
/// (main agent and subagents) gets a fresh instance per build.
pub type LocalToolsFn = Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>;

/// Assemble the engine-owned MAIN-AGENT tool set: DAG tools, the `subagent` delegation
/// tool, the skill family, and memory. The app layer calls this and appends its local
/// tools (and process-level groups like MCP).
#[allow(clippy::too_many_arguments)]
pub fn engine_tools(
    memory_dir: &Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    subagent_tools: SubagentToolsFn,
    launch_resolver: LaunchResolver,
    spec_names: Vec<String>,
    model: &Model,
    stream_fn: Option<&StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = Vec::new();
    // DAG tools (session-stamped: dag_* refuse runs owned by another session).
    tools.extend(dag_tools::DagTools::new(
        dag_engine.clone(),
        Some(session_id.to_string()),
        spec_names.clone(),
    ));
    // Subagent delegation tool: shares the parent's model + stream backend; jobs are
    // stamped with this session.
    tools.push(Arc::new(
        SubagentTool::new(
            model.clone(),
            stream_fn.cloned(),
            subagent_tools,
            launch_resolver,
            spec_names,
            subagent_registry.clone(),
        )
        .with_session_id(Some(session_id.to_string())),
    ));
    // Skill family — each wires a fresh harness cell per harness build.
    tools.extend(skill_family(skill_harness_cell));
    // Memory: same dir as the parent's store.
    tools.push(Arc::new(MemoryTool::new(memory_dir.to_path_buf())));
    tools
}

/// Engine tools a SUBAGENT may have: everything except the two orchestration tools —
/// no `subagent` (no recursive delegation) and no `dag_*` (no DAG orchestration from
/// inside a subagent). Skills + memory are engine capabilities a subagent may use.
pub fn subagent_engine_tools(
    memory_dir: &Path,
    skill_harness_cell: &SkillHarnessCell,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = skill_family(skill_harness_cell);
    tools.push(Arc::new(MemoryTool::new(memory_dir.to_path_buf())));
    tools
}

/// Build the ONE subagent tool-set resolver: every spec (explorer / planner /
/// executor-coder / checker / general) gets the same uniform set = engine tools minus
/// `subagent`/`dag_*` ([`subagent_engine_tools`]) plus the app's local tools. Shared by
/// the `subagent` tool and the DAG node launcher.
pub fn subagent_tools(
    memory_dir: &Path,
    skill_harness_cell: &SkillHarnessCell,
    local_tools: LocalToolsFn,
) -> ToolSetResolver {
    let memory_dir = memory_dir.to_path_buf();
    let cell = skill_harness_cell.clone();
    Arc::new(move |_spec_name: &str| {
        let mut tools = subagent_engine_tools(&memory_dir, &cell);
        tools.extend(local_tools());
        tools
    })
}

/// The skill family: skill / install / builder / state / remove, all wired to the same
/// harness cell.
fn skill_family(skill_harness_cell: &SkillHarnessCell) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(skill::SkillTool::new(skill_harness_cell.clone())),
        Arc::new(install_skill::InstallSkillTool::new(
            skill_harness_cell.clone(),
        )),
        Arc::new(skill_builder::SkillBuilderTool::new(
            skill_harness_cell.clone(),
        )),
        Arc::new(set_skill_state::SetSkillStateTool::new(
            skill_harness_cell.clone(),
        )),
        Arc::new(remove_skill::RemoveSkillTool::new(
            skill_harness_cell.clone(),
        )),
    ]
}
