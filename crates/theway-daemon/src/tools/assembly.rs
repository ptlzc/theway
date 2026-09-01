//! `assembly` — engine-side tool assembly for the harness-runtime tools.
//!
//! The tools the ENGINE itself owns: DAG orchestration (`dag_*`), the `subagent`
//! delegation tool, the skill family (skill / install / builder / state / remove), and
//! memory. These are runtime capabilities — they do not depend on the execution
//! environment — so the engine knows how to wire them. The local-execution tools
//! (bash / fs / git / grep / web) are supplied alongside as a factory.
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

use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_core::multiagent::types::ToolSetResolver;
use theway_core::{AgentTool, StreamFn};

use super::dag_tools;
use super::skill::{self, SkillHarnessCell};
use super::subagent::{SubagentTool, SubagentToolsFn};
use theway_core::multiagent::types::AgentRunResolver;

// Direct-FS-write tool bodies are only referenced from the `local` registration path.
#[cfg(feature = "local")]
use super::install_skill;
#[cfg(feature = "local")]
use super::memory::MemoryTool;
#[cfg(feature = "local")]
use super::remove_skill;
#[cfg(feature = "local")]
use super::set_skill_state;
#[cfg(feature = "local")]
use super::skill_builder;

/// Engine-owned tool names whose bodies write straight to the host filesystem outside
/// the [`theway_core::executor::ToolExecutor`] seam:
/// - `memory` — `tokio::fs` reads/writes under the memory dir;
/// - `install_skill` / `skill_builder` — atomic file writes into `~/.theway/skills`;
/// - `set_skill_state` — writes the `~/.theway/skill-overrides.json` overlay;
/// - `remove_skill` — `tokio::fs` dir/file removal under `~/.theway/skills`.
///
/// Registered ONLY in `local` builds; sandbox-only builds omit them (fail closed,
/// issue #64) and name every omission via `tracing::warn`. The read-only `skill`
/// lookup (in-memory catalog snapshot) and `reload` (harness-level rescan) stay
/// registered in every build.
pub const LOCAL_ONLY_ENGINE_TOOL_NAMES: &[&str] = &[
    "memory",
    "install_skill",
    "skill_builder",
    "set_skill_state",
    "remove_skill",
];

/// Sandbox-only note (issue #64): one explicit `tracing::warn` per tool-set assembly
/// naming every engine-owned direct-FS-write tool that was left unregistered.
#[cfg(all(not(feature = "local"), feature = "sandbox"))]
fn warn_sandbox_omitted_engine_tools() {
    tracing::warn!(
        omitted = ?LOCAL_ONLY_ENGINE_TOOL_NAMES,
        "sandbox-only build: engine tools that write directly to the host FS are NOT \
         registered (fail closed); the in-memory `skill` lookup and `reload` remain"
    );
}

// The reload tool body lives flat in `src/tools/reload.rs` next to the other
// tool bodies; the `#[path]` anchor keeps that file layout.
#[path = "reload.rs"]
pub mod reload;

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
    base_dir: &Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    subagent_tools: SubagentToolsFn,
    launch_resolver: AgentRunResolver,
    spec_names: Vec<String>,
    model: Option<&Model>,
    stream_fn: Option<&StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
    reload_runtime: reload::ReloadRuntimeSlot,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = Vec::new();
    // DAG tools (session-stamped: dag_* refuse runs owned by another session).
    tools.extend(dag_tools::DagTools::new(
        dag_engine.clone(),
        Some(session_id.to_string()),
        spec_names.clone(),
        subagent_registry.clone(),
    ));
    // Subagent delegation tool: shares the parent's model + stream backend; jobs are
    // stamped with this session.
    tools.push(Arc::new(
        SubagentTool::new(
            model.cloned(),
            stream_fn.cloned(),
            subagent_tools,
            launch_resolver,
            spec_names,
            subagent_registry.clone(),
        )
        .with_session_id(Some(session_id.to_string())),
    ));
    // Skill family — each wires a fresh harness cell per harness build. In
    // sandbox-only builds the direct-FS-write members are left unregistered
    // (see `skill_family`).
    tools.extend(skill_family(skill_harness_cell, base_dir));
    // Reload: the LLM's entry point for rescan semantics. The application-owned
    // runtime slot is bound after TurnHost construction.
    tools.push(Arc::new(reload::ReloadTool::new(
        skill_harness_cell.clone(),
        reload_runtime,
    )));
    // Memory: same dir as the parent's store. Direct `tokio::fs` writes — gated
    // the same way as the skill writers (issue #64).
    push_memory_tool(&mut tools, memory_dir);
    #[cfg(all(not(feature = "local"), feature = "sandbox"))]
    warn_sandbox_omitted_engine_tools();
    tools
}

/// Engine tools a SUBAGENT may have: everything except the two orchestration tools —
/// no `subagent` (no recursive delegation) and no `dag_*` (no DAG orchestration from
/// inside a subagent). Skills + memory are engine capabilities a subagent may use.
/// `base_dir` is the theway base dir (issue #66: `DaemonPaths::base`), wired into
/// the direct-FS-write skill family.
pub fn subagent_engine_tools(
    memory_dir: &Path,
    base_dir: &Path,
    skill_harness_cell: &SkillHarnessCell,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = skill_family(skill_harness_cell, base_dir);
    push_memory_tool(&mut tools, memory_dir);
    #[cfg(all(not(feature = "local"), feature = "sandbox"))]
    warn_sandbox_omitted_engine_tools();
    tools
}

/// Append the `memory` tool in `local` builds only. Its body reads/writes the memory
/// dir with `tokio::fs` directly (no [`theway_core::executor::ToolExecutor`] seam), so
/// sandbox-only builds leave it unregistered (issue #64, fail closed); the omission is
/// covered by the assembly-level warn next to the call sites.
#[cfg(feature = "local")]
fn push_memory_tool(tools: &mut Vec<Arc<dyn AgentTool>>, memory_dir: &Path) {
    tools.push(Arc::new(MemoryTool::new(memory_dir.to_path_buf())));
}

#[cfg(not(feature = "local"))]
fn push_memory_tool(tools: &mut Vec<Arc<dyn AgentTool>>, memory_dir: &Path) {
    let _ = (tools, memory_dir);
}

/// Build the ONE subagent tool-set resolver: every spec (explorer / planner /
/// executor-coder / checker / general) gets the same uniform set = engine tools minus
/// `subagent`/`dag_*` ([`subagent_engine_tools`]) plus the app's local tools. Shared by
/// the `subagent` tool and the DAG node launcher. `base_dir` is the theway base dir
/// (issue #66: `DaemonPaths::base`) for the skill family's host paths.
pub fn subagent_tools(
    memory_dir: &Path,
    base_dir: &Path,
    skill_harness_cell: &SkillHarnessCell,
    local_tools: LocalToolsFn,
) -> ToolSetResolver {
    let memory_dir = memory_dir.to_path_buf();
    let base_dir = base_dir.to_path_buf();
    let cell = skill_harness_cell.clone();
    Arc::new(move |_spec_name: &str| {
        let mut tools = subagent_engine_tools(&memory_dir, &base_dir, &cell);
        tools.extend(local_tools());
        tools
    })
}

/// The skill family: skill / install / builder / state / remove, all wired to the same
/// harness cell.
///
/// Host paths (issue #66): `base_dir` is the theway base dir resolved once at the CLI
/// boundary (`DaemonPaths::base`); the direct-FS-write members are constructed with
/// their explicit-path variants (`with_skills_root` / `with_base_dir`) so no tool in
/// this set reads `THEWAY_DIR` / `HOME` at construction time. `base_dir.join("skills")`
/// mirrors `DaemonPaths::skills_root()`.
///
/// Feature gating (issue #64, fail closed): `skill` is a pure in-memory catalog lookup
/// and stays registered in every build. The other four write the host filesystem
/// directly (`~/.theway/skills/**`, `~/.theway/skill-overrides.json`) without going
/// through the executor seam, so sandbox-only builds leave them unregistered; the
/// assembly-level `warn_sandbox_omitted_engine_tools` names them.
#[cfg_attr(not(feature = "local"), allow(unused_variables))]
fn skill_family(skill_harness_cell: &SkillHarnessCell, base_dir: &Path) -> Vec<Arc<dyn AgentTool>> {
    // `mut` is only needed on the `local` path, which extends the set below.
    #[cfg_attr(not(feature = "local"), allow(unused_mut))]
    let mut tools: Vec<Arc<dyn AgentTool>> =
        vec![Arc::new(skill::SkillTool::new(skill_harness_cell.clone()))];

    #[cfg(feature = "local")]
    {
        let skills_root = base_dir.join("skills");
        tools.extend([
            Arc::new(install_skill::InstallSkillTool::with_skills_root(
                skill_harness_cell.clone(),
                skills_root.clone(),
            )) as Arc<dyn AgentTool>,
            Arc::new(skill_builder::SkillBuilderTool::with_skills_root(
                skill_harness_cell.clone(),
                skills_root,
            )),
            Arc::new(set_skill_state::SetSkillStateTool::with_base_dir(
                skill_harness_cell.clone(),
                base_dir.to_path_buf(),
            )),
            Arc::new(remove_skill::RemoveSkillTool::with_base_dir(
                skill_harness_cell.clone(),
                base_dir.to_path_buf(),
            )),
        ]);
    }

    tools
}
