//! `assembly` — engine-side tool assembly for the harness-runtime tools.
//!
//! Mirrors the app-layer assembly in the `theway` server crate, but for the tools the
//! ENGINE itself owns: DAG orchestration (`dag_*`), the `task` subagent tool, the skill
//! family (skill / install / builder / state / remove), and memory. These are runtime
//! capabilities — they do not depend on the execution environment — so the engine knows
//! how to wire them. The app layer only adds its local-execution tools
//! (bash / fs / git / grep / web) on top of [`session_engine_tools`].
//!
//! The single app-layer injection point is the subagent tool-set resolver ([`SubagentToolsFn`]):
//! the engine cannot know which tools exist, so the app supplies the resolver and the
//! engine hands it to both the `task` tool and (via [`super::node_launcher`]) the DAG
//! launcher — one mechanism, one resolver instance.
//!
//! # Skill harness-cell timing
//! The skill family needs the live `AgentHarness::skills()` snapshot, and the harness
//! does not exist yet when this is called (we are still assembling the tool list that
//! will be passed to `AgentHarness::new`). The caller builds an
//! `Arc<OnceCell<Arc<AgentHarness>>>`, passes it in, and — crucially — sets the cell
//! immediately after the harness is constructed and *before* the REPL accepts any input.
//! If the cell is unset at execute time the tools return a recoverable `AgentToolError`,
//! never a panic.

use std::path::Path;
use std::sync::Arc;

use theway_llm_provider::Model;

use crate::runtime::graph_engineering::engine::DagEngine;
use crate::runtime::subagents::registry::SubagentJobRegistry;
use crate::{AgentTool, StreamFn};

use super::dag_tools;
use super::install_skill;
use super::memory::MemoryTool;
use super::remove_skill;
use super::set_skill_state;
use super::skill::{self, SkillHarnessCell};
use super::skill_builder;
use super::task::{SubagentToolsFn, TaskTool};

/// Assemble the engine-owned session tool set: DAG tools, the `task` delegation tool,
/// the skill family, and memory. The app layer calls this and appends its local tools.
#[allow(clippy::too_many_arguments)]
pub fn session_engine_tools(
    memory_dir: &Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    subagent_tools: SubagentToolsFn,
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
    ));
    // Task delegation tool: shares the parent's model + stream backend; jobs are
    // stamped with this session.
    tools.push(Arc::new(
        TaskTool::new(
            model.clone(),
            stream_fn.cloned(),
            subagent_tools,
            subagent_registry.clone(),
        )
        .with_session_id(Some(session_id.to_string())),
    ));
    // Skill family — each wires a fresh harness cell per harness build.
    tools.push(Arc::new(skill::SkillTool::new(skill_harness_cell.clone())));
    tools.push(Arc::new(install_skill::InstallSkillTool::new(
        skill_harness_cell.clone(),
    )));
    tools.push(Arc::new(skill_builder::SkillBuilderTool::new(
        skill_harness_cell.clone(),
    )));
    tools.push(Arc::new(set_skill_state::SetSkillStateTool::new(
        skill_harness_cell.clone(),
    )));
    tools.push(Arc::new(remove_skill::RemoveSkillTool::new(
        skill_harness_cell.clone(),
    )));
    // Memory: same dir as the parent's store.
    tools.push(Arc::new(MemoryTool::new(memory_dir.to_path_buf())));
    tools
}
