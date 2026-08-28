//! Tool ASSEMBLY layer — the application-layer half of the session tool set.
//!
//! All tool BODIES (harness-runtime tools AND local-execution tools) live HERE, in
//! the daemon (daemon-kernel-layers: the daemon is the single kernel; the engine
//! crate `theway_core` keeps only the `AgentTool` / `ToolExecutor` / `ExecutionEnv`
//! trait families and the agent runtime):
//!
//! - **Harness-runtime tools** (graph/DAG, subagents, skills, memory, MCP — formerly
//!   `theway_core::tools`) sit next to the local bodies: `assembly` / `subagent` /
//!   `dag_tools` / the skill family / `memory` / `mcp_adapter` / `exec_shell`.
//! - **Local-execution tool bodies** (bash / fs / git / grep / find / ls / outline /
//!   truncate) are environment-specific agent capabilities.
//! - **Web tools** (`web_fetch` / `web_search`) are app-layer capabilities too (they
//!   need external credentials/configuration).
//! - **Assembly**: [`local_tools`] / the subagent tool-set resolver / `session_tool_set`
//!   wire engine tools + local tools together, then append the server-side
//!   trigger/cron family.
//!
//! Subagent tool sets are injected into the engine (specs carry no tool
//! factory): [`subagent_tool_sets`] builds the ONE resolver both `subagent_tool` and the
//! DAG node launcher are constructed with — every spec gets the same uniform set (engine
//! tools minus `subagent`/`dag_*` plus local tools); behavior is prompt-defined.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use theway_core::executor::ToolExecutor;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::node_launcher;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_core::multiagent::types::ToolSetResolver;
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, PermissionClassification,
};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use crate::runtime_storage::SessionRepository;
use crate::tools::skill::SkillHarnessCell;

// ── tool bodies (harness-runtime + app-layer) ───────────────────────────────
//
// Plain module declarations: `tests/tools.rs` pulls in only `triggers/tool_assembly.rs`
// by `#[path]`, so nothing here needs `#[path]` re-routing — a plain `pub mod bash;`
// resolves `src/tools/bash.rs` and lets `bash.rs`'s own `mod tests;` find
// `src/tools/bash/tests/` (a `#[path]`-included module would break that resolution).

// Harness-runtime tools (moved from theway-core, daemon-kernel-layers).
pub mod assembly;
pub mod dag_tools;
pub mod exec;
pub mod exec_shell;
pub mod install_skill;
pub mod mcp_adapter;
pub mod memory;
pub mod remove_skill;
pub mod session_graph;
pub mod set_skill_state;
pub mod skill;
pub mod skill_builder;
pub mod subagent;

// App-layer local-execution + web tools.
pub mod bash;
pub mod edit;
pub mod find;
pub mod git;
pub mod grep;
pub mod ls;
pub mod outline;
pub mod read;
pub mod truncate;
pub mod web_fetch;
pub mod web_search;
pub mod write;

// Trigger/cron model-facing constructors live in `triggers::tool_assembly` (kept
// out of this file so `tests/tools.rs` can `#[path]`-include just those).
pub use crate::triggers::tool_assembly::{
    list_cron_jobs_tool, list_triggers_tool, new_cron_job_tool, new_trigger_tool,
    remove_cron_job_tool, remove_trigger_tool, set_cron_job_state_tool, set_trigger_state_tool,
};

/// Tool names that bypass the [`ToolExecutor`] seam and touch the host OS directly:
/// `bash` spawns `sh -c` process groups (setsid/killpg), the `exec_shell` family owns
/// `tokio::process` children, and `ls` / `grep` / `find` walk the local filesystem
/// with `tokio::fs` / `ignore`. They are registered ONLY in `local` builds. In
/// sandbox-only builds they are omitted from the tool set (fail closed, issue #64) —
/// the [`crate::executor::sandbox::SandboxExecutor`] seam covers only the
/// executor-backed tools, so these bodies would otherwise keep acting straight on the
/// host even in sandbox mode.
pub const LOCAL_ONLY_TOOL_NAMES: &[&str] = &[
    "bash",
    "exec",
    "get_output",
    "kill_shell",
    "write_to_process",
    "ls",
    "grep",
    "find",
];

/// Scopes a direct-OS tool to an owning cwd unless the caller supplies one.
pub struct CwdScopedTool {
    inner: Arc<dyn AgentTool>,
    cwd: PathBuf,
}

impl CwdScopedTool {
    pub fn new(inner: Arc<dyn AgentTool>, cwd: PathBuf) -> Self {
        Self { inner, cwd }
    }

    fn scope_args(&self, mut args: Value) -> Value {
        match self.inner.definition().name.as_str() {
            "bash" | "exec" | "ls" | "grep" | "find" if args.get("cwd").is_none() => {
                if let Some(obj) = args.as_object_mut() {
                    obj.insert("cwd".into(), self.cwd.to_string_lossy().into_owned().into());
                }
            }
            _ => {}
        }
        args
    }
}

#[async_trait]
impl AgentTool for CwdScopedTool {
    fn definition(&self) -> &Tool {
        self.inner.definition()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn execution_mode(&self) -> Option<theway_core::ToolExecutionMode> {
        self.inner.execution_mode()
    }

    fn prepare_arguments(&self, args: Value) -> Value {
        self.inner.prepare_arguments(self.scope_args(args))
    }

    fn permission_classification(&self, prepared_args: &Value) -> PermissionClassification {
        self.inner.permission_classification(prepared_args)
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancel: CancellationToken,
        on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        self.inner
            .execute(tool_call_id, self.scope_args(params), cancel, on_update)
            .await
    }
}

/// Compatibility wrapper that uses the process cwd as the owning cwd.
#[cfg(feature = "local")]
pub fn local_tools(executor: Arc<dyn ToolExecutor>) -> Vec<Arc<dyn AgentTool>> {
    local_tools_for_cwd(executor, std::env::current_dir().unwrap_or_default())
}

/// Local-execution tool set (the app-layer half of the session tool set): shell / fs /
/// git / grep / web — everything that depends on the execution environment. No engine
/// tools here (DAG / subagent / skills / memory come from the kernel's own assembly,
/// [`crate::tools::assembly`]); no duplicate-memory risk.
///
/// Executor binding (sdk-split-local-sandbox node 8): file-content and process tools
/// (read / write / edit / outline / git) dispatch their effects through the injected
/// [`ToolExecutor`] — the local executor for local editing mode; a sandbox
/// executor swaps the execution environment without touching tool definitions. The
/// remaining local tools are not yet wired through the executor: `bash` keeps its
/// process-group-kill + cancel semantics (the executor's `run_command` kills only the
/// direct child), and `ls` / `grep` / `find` / the `exec_shell` family use richer
/// directory/walk surfaces than the executor trait's first cut exposes.
///
/// **Feature gating (issue #64, fail closed)**: in sandbox-only builds
/// (`not(local) + sandbox`) the direct-OS tools ([`LOCAL_ONLY_TOOL_NAMES`]) are NOT
/// registered and a `tracing::warn` names every omitted tool — never a silent drop.
/// The executor-backed tools (read / write / edit / outline / git) stay registered:
/// their effects go through the [`ToolExecutor`] seam, where the sandbox executor
/// answers with an explicit `UnsupportedKind` error. `web_fetch` / `web_search` stay
/// too: they are pure network requests with no host FS/process side effects.
#[cfg(feature = "local")]
pub fn local_tools_for_cwd(
    executor: Arc<dyn ToolExecutor>,
    cwd: PathBuf,
) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool::new(executor.clone())),
        Arc::new(write::WriteTool::new(executor.clone())),
        Arc::new(edit::EditTool::new(executor.clone())),
        Arc::new(CwdScopedTool::new(Arc::new(bash::BashTool), cwd.clone())),
        Arc::new(CwdScopedTool::new(
            Arc::new(exec_shell::ExecTool),
            cwd.clone(),
        )),
        Arc::new(exec_shell::GetOutputTool),
        Arc::new(exec_shell::KillShellTool),
        Arc::new(exec_shell::WriteToProcessTool),
        Arc::new(CwdScopedTool::new(Arc::new(ls::LsTool), cwd.clone())),
        Arc::new(CwdScopedTool::new(Arc::new(grep::GrepTool), cwd.clone())),
        Arc::new(CwdScopedTool::new(Arc::new(find::FindTool), cwd)),
        Arc::new(outline::OutlineTool::new(executor.clone())),
        Arc::new(git::GitTool::new(executor)),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(web_search::WebSearchTool::new()),
    ]
}

/// Sandbox-only variant: only the executor-backed tools and the network-only web tools
/// are registered (see the `local` variant's doc for the policy). The omitted
/// direct-OS tools are named explicitly in a `tracing::warn` so the degraded tool set
/// is never silent.
#[cfg(all(not(feature = "local"), feature = "sandbox"))]
pub fn local_tools(executor: Arc<dyn ToolExecutor>) -> Vec<Arc<dyn AgentTool>> {
    local_tools_for_cwd(executor, std::env::current_dir().unwrap_or_default())
}

#[cfg(all(not(feature = "local"), feature = "sandbox"))]
pub fn local_tools_for_cwd(
    executor: Arc<dyn ToolExecutor>,
    _cwd: PathBuf,
) -> Vec<Arc<dyn AgentTool>> {
    tracing::warn!(
        omitted = ?LOCAL_ONLY_TOOL_NAMES,
        "sandbox-only build: local-only tools bypass the ToolExecutor seam and touch \
         the host FS/process table directly, so they are NOT registered (fail closed); \
         executor-backed tools (read/write/edit/outline/git) and network-only tools \
         (web_fetch/web_search) remain"
    );
    vec![
        Arc::new(read::ReadTool::new(executor.clone())),
        Arc::new(write::WriteTool::new(executor.clone())),
        Arc::new(edit::EditTool::new(executor.clone())),
        Arc::new(outline::OutlineTool::new(executor.clone())),
        Arc::new(git::GitTool::new(executor)),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(web_search::WebSearchTool::new()),
    ]
}

/// Fails the build when neither execution backend is selected. Mirrors
/// [`crate::executor::default_executor`]: a daemon without any executor backend has no
/// valid tool execution story at all.
#[cfg(not(any(feature = "local", feature = "sandbox")))]
pub fn local_tools(_executor: Arc<dyn ToolExecutor>) -> Vec<Arc<dyn AgentTool>> {
    compile_error!("theway-daemon requires at least one of the `local` or `sandbox` features");
    #[allow(unreachable_code)]
    unreachable!()
}

/// Build the `Subagent` tool. Separate from `local_tools` because the tool needs the model handle to
/// spawn its inner harness; the caller wires it in at construction time. `session_id`
/// (session-resource-model) stamps the owning session on every spawned job — each harness
/// build gets its own SubagentTool stamped with that harness's session.
///
/// The subagent tool set comes from [`subagent_tool_sets`] — the SAME resolver the DAG
/// node launcher gets (one uniform set per spec: engine tools minus subagent/dag_* plus
/// local tools). The main agent picks the spec (explorer / planner / executor-coder /
/// checker / general) and defines in the prompt what the subagent may do.
pub fn subagent_tool(
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    registry: SubagentJobRegistry,
    memory_dir: PathBuf,
    base_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    session_id: Option<String>,
    executor: Arc<dyn ToolExecutor>,
) -> Arc<dyn AgentTool> {
    Arc::new(
        subagent::SubagentTool::new(
            model,
            stream_fn,
            subagent_tool_sets(memory_dir, base_dir, skill_harness_cell, executor),
            crate::agent_specs::launch_resolver(),
            crate::agent_specs::spec_names(),
            registry,
        )
        .with_session_id(session_id),
    )
}

/// Build the app-layer tool-set resolver injected into the subagent tool and the DAG
/// node launcher. ONE uniform set for every spec: engine tools minus the two
/// orchestration tools (`subagent` / `dag_*`) plus local tools — assembled kernel-side,
/// [`crate::tools::assembly::subagent_tools`]. Per-spec tool differences are gone;
/// behavior is defined by the spec's system prompt and the parent's task prompt.
/// `base_dir` is the theway base dir (issue #66: `DaemonPaths::base`) for the skill
/// family's host paths.
pub fn subagent_tool_sets(
    memory_dir: PathBuf,
    base_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    executor: Arc<dyn ToolExecutor>,
) -> ToolSetResolver {
    subagent_tool_sets_for_cwd(
        memory_dir,
        base_dir,
        skill_harness_cell,
        executor,
        std::env::current_dir().unwrap_or_default(),
    )
}

pub fn subagent_tool_sets_for_cwd(
    memory_dir: PathBuf,
    base_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    executor: Arc<dyn ToolExecutor>,
    cwd: PathBuf,
) -> ToolSetResolver {
    assembly::subagent_tools(
        &memory_dir,
        &base_dir,
        &skill_harness_cell,
        // The kernel-side local-tools factory closes over the daemon's executor and cwd,
        // so every subagent / DAG-node tool set dispatches through the same execution
        // environment and path-scoped direct-OS tools.
        Arc::new(move || local_tools_for_cwd(executor.clone(), cwd.clone())),
    )
}

/// Build a DAG node launcher wired to `engine`, with the app-layer tool-set resolver.
/// `base_dir` is the theway base dir (issue #66: `DaemonPaths::base`) for the skill
/// family's host paths.
pub fn node_launcher(
    engine: Arc<DagEngine>,
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    cwd: PathBuf,
    registry: SubagentJobRegistry,
    memory_dir: PathBuf,
    base_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    executor: Arc<dyn ToolExecutor>,
) -> Arc<node_launcher::NodeLauncherImpl> {
    node_launcher::node_launcher(
        engine,
        model,
        stream_fn,
        cwd.clone(),
        registry,
        subagent_tool_sets_for_cwd(memory_dir, base_dir, skill_harness_cell, executor, cwd),
        crate::agent_specs::launch_resolver(),
    )
}

/// Build the per-session tool set (session-resource-model). One source of truth shared by
/// the CLI's initial harness build and the session factory ([`crate::session_ops::SessionFactory`]):
/// everything here is either session-stamped (`dag_*` / `subagent`) or must be rebuilt per
/// harness (the skill family wires a fresh harness cell per build). Local tools
/// ([`local_tools`]) + engine tools ([`crate::tools::assembly::engine_tools`]) +
/// the server-side trigger/cron family. Process-level tool groups (MCP tools) are the
/// caller's to add.
pub fn session_tool_set(
    memory_dir: &std::path::Path,
    base_dir: &std::path::Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    model: &theway_llm_provider::Model,
    stream_fn: Option<&theway_core::StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
    executor: Arc<dyn ToolExecutor>,
    services: &crate::DaemonServices,
    repo: Arc<dyn SessionRepository>,
) -> Vec<Arc<dyn AgentTool>> {
    session_tool_set_for_cwd(
        memory_dir,
        base_dir,
        dag_engine,
        subagent_registry,
        model,
        stream_fn,
        skill_harness_cell,
        session_id,
        executor,
        services,
        repo,
        std::env::current_dir().unwrap_or_default(),
    )
}

pub fn session_tool_set_for_cwd(
    memory_dir: &std::path::Path,
    base_dir: &std::path::Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    model: &theway_llm_provider::Model,
    stream_fn: Option<&theway_core::StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
    executor: Arc<dyn ToolExecutor>,
    services: &crate::DaemonServices,
    repo: Arc<dyn SessionRepository>,
    cwd: PathBuf,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = local_tools_for_cwd(executor.clone(), cwd.clone());
    // Engine-owned tools (DAG / subagent / skills / memory), assembled kernel-side with the
    // same subagent tool-set resolver the DAG node launcher uses.
    tools.extend(assembly::engine_tools(
        memory_dir,
        base_dir,
        dag_engine,
        subagent_registry,
        subagent_tool_sets_for_cwd(
            memory_dir.to_path_buf(),
            base_dir.to_path_buf(),
            skill_harness_cell.clone(),
            executor,
            cwd.clone(),
        ),
        crate::agent_specs::launch_resolver(),
        crate::agent_specs::spec_names(),
        model,
        stream_fn,
        skill_harness_cell,
        session_id,
        services.reload.clone(),
    ));
    // Session graph tools (main-agent only): list/read/status/wait/attach against
    // the Turso-backed session graph.
    let graph_path = theway_contract::config::sessions_dir_for_cwd(&cwd).join("session_graph.db");
    tools.extend(session_graph::SessionGraphTools::new(
        repo.clone(),
        graph_path,
        cwd.clone(),
    ));
    // Trigger/cron family: harness-adjacent but implemented in this crate.
    tools.push(new_cron_job_tool(
        skill_harness_cell.clone(),
        services.cron.clone(),
    ));
    tools.push(list_cron_jobs_tool(services.cron.clone()));
    tools.push(remove_cron_job_tool(
        skill_harness_cell.clone(),
        services.cron.clone(),
    ));
    tools.push(set_cron_job_state_tool(
        skill_harness_cell.clone(),
        services.cron.clone(),
    ));
    tools.push(new_trigger_tool(services.dynamic_triggers.clone()));
    tools.push(list_triggers_tool(services.dynamic_triggers.clone()));
    tools.push(remove_trigger_tool(services.dynamic_triggers.clone()));
    tools.push(set_trigger_state_tool(services.dynamic_triggers.clone()));
    tools
}

#[cfg(test)]
// Test files live in `tests/tools/mod.rs` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("tools");
