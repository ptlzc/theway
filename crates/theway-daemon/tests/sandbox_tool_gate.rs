//! Sandbox-only tool-set gating (issue #64, fail closed).
//!
//! In a build without the `local` feature the daemon must NOT register any tool that
//! bypasses the [`theway_core::executor::ToolExecutor`] seam and touches the host OS
//! directly (process table / filesystem). These tests call the REAL assembly functions
//! — [`theway_daemon::tools::local_tools`], [`theway_daemon::tools::session_tool_set`],
//! [`theway_daemon::tools::assembly::engine_tools`] and the subagent tool-set resolver —
//! and assert:
//!
//! 1. The direct-OS tools (`bash`, `exec`/`get_output`/`kill_shell`/`write_to_process`,
//!    `ls`, `grep`, `find`) and the direct-FS-write engine tools (`memory`,
//!    `install_skill`, `skill_builder`, `set_skill_state`, `remove_skill`) are absent
//!    from every assembled name set.
//! 2. The executor-backed tools (`read` / `write` / `edit` / `outline` / `git`) stay
//!    registered: their effects dispatch through the [`theway_core::executor::ToolExecutor`]
//!    seam, where [`theway_daemon::executor::sandbox::SandboxExecutor`] answers with an
//!    explicit `UnsupportedKind(Sandbox)` error at call time (fail closed per call, not
//!    a silent empty set). The network-only `web_fetch` / `web_search`, the in-memory
//!    `skill` lookup, `reload`, the DAG/subagent orchestration and the in-memory
//!    trigger/cron family remain for the same reason.
//! 3. An executor-backed call actually fails closed with the unsupported-kind error.

#![cfg(all(not(feature = "local"), feature = "sandbox"))]

use std::collections::HashSet;
use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::executor::{ExecutorKind, ToolExecutor};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_daemon::executor::sandbox::SandboxExecutor;
use theway_daemon::tools;
use theway_daemon::tools::assembly;
use theway_daemon::tools::skill::SkillHarnessCell;
use tokio_util::sync::CancellationToken;

/// Direct-OS local tools: bypass the executor seam (process table / FS walk).
const DIRECT_OS_LOCAL_TOOLS: &[&str] = &[
    "bash",
    "exec",
    "get_output",
    "kill_shell",
    "write_to_process",
    "ls",
    "grep",
    "find",
];

/// Engine-owned tools that write the host FS directly (memory dir, `~/.theway/skills`,
/// `skill-overrides.json`).
const DIRECT_OS_ENGINE_TOOLS: &[&str] = &[
    "memory",
    "install_skill",
    "skill_builder",
    "set_skill_state",
    "remove_skill",
];

/// Executor-backed file/git tools — must stay registered; the sandbox executor rejects
/// each call with `UnsupportedKind(Sandbox)`.
const EXECUTOR_BACKED_TOOLS: &[&str] = &["read", "write", "edit", "outline", "git"];

/// Network-only (no host FS/process side effects) — stay registered.
const NETWORK_ONLY_TOOLS: &[&str] = &["web_fetch", "web_search"];

fn sandbox_exec() -> Arc<dyn ToolExecutor> {
    Arc::new(SandboxExecutor::new())
}

fn names(tools: &[Arc<dyn AgentTool>]) -> HashSet<String> {
    tools.iter().map(|t| t.definition().name.clone()).collect()
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
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn harness_cell() -> SkillHarnessCell {
    Arc::new(once_cell::sync::OnceCell::new())
}

#[tokio::test]
async fn default_executor_reports_sandbox_kind() {
    let executor = theway_daemon::executor::default_executor();
    assert_eq!(executor.kind().await, ExecutorKind::Sandbox);
}

#[test]
fn local_tools_omits_every_direct_os_tool() {
    let names = names(&tools::local_tools(sandbox_exec()));

    for tool in DIRECT_OS_LOCAL_TOOLS {
        assert!(
            !names.contains(*tool),
            "sandbox-only build must not register direct-OS tool `{tool}`; got {names:?}"
        );
    }
    // Belt and braces against a silently emptied set: the documented remainder is exact.
    assert_eq!(
        names,
        HashSet::from_iter(
            EXECUTOR_BACKED_TOOLS
                .iter()
                .chain(NETWORK_ONLY_TOOLS)
                .map(|s| s.to_string())
        ),
        "sandbox-only local_tools must be exactly the executor-backed + network-only set"
    );
}

#[test]
fn engine_tools_omit_direct_fs_writers_but_keep_read_only_skill_surface() {
    let dag_engine = Arc::new(DagEngine::new());
    let registry = SubagentJobRegistry::new();
    let model = faux_model();
    let tools = assembly::engine_tools(
        std::path::Path::new("/nonexistent-memory-dir"),
        std::path::Path::new("/nonexistent-theway-base"),
        &dag_engine,
        &registry,
        Arc::new(move |_spec: &str| Vec::<Arc<dyn AgentTool>>::new()),
        theway_daemon::agent_specs::launch_resolver(),
        theway_daemon::agent_specs::spec_names(),
        &model,
        None,
        &harness_cell(),
        "session-sandbox-gate",
    );
    let names = names(&tools);

    for tool in DIRECT_OS_ENGINE_TOOLS {
        assert!(
            !names.contains(*tool),
            "sandbox-only build must not register direct-FS-write engine tool `{tool}`; got {names:?}"
        );
    }
    // The read-only in-memory skill lookup and the harness-level reload stay; so does
    // the DAG orchestration family (session-stamped, in-memory).
    for tool in ["skill", "reload", "subagent", "dag_plan", "dag_status"] {
        assert!(
            names.contains(tool),
            "engine tool `{tool}` must remain registered in sandbox-only builds; got {names:?}"
        );
    }
}

#[test]
fn session_tool_set_assembly_is_fail_closed() {
    let dag_engine = Arc::new(DagEngine::new());
    let registry = SubagentJobRegistry::new();
    let model = faux_model();
    let tools = tools::session_tool_set(
        std::path::Path::new("/nonexistent-memory-dir"),
        std::path::Path::new("/nonexistent-theway-base"),
        &dag_engine,
        &registry,
        &model,
        None,
        &harness_cell(),
        "session-sandbox-gate",
        sandbox_exec(),
    );
    let names = names(&tools);

    // Never a silent empty set.
    assert!(
        names.len() >= EXECUTOR_BACKED_TOOLS.len() + NETWORK_ONLY_TOOLS.len(),
        "session tool set collapsed unexpectedly: {names:?}"
    );

    for tool in DIRECT_OS_LOCAL_TOOLS.iter().chain(DIRECT_OS_ENGINE_TOOLS) {
        assert!(
            !names.contains(*tool),
            "sandbox-only session tool set must not contain direct-OS tool `{tool}`; got {names:?}"
        );
    }

    // Executor-backed + network-only local tools remain.
    for tool in EXECUTOR_BACKED_TOOLS.iter().chain(NETWORK_ONLY_TOOLS) {
        assert!(
            names.contains(*tool),
            "`{tool}` must remain registered in sandbox-only builds; got {names:?}"
        );
    }

    // The in-memory control surface stays available (scheduling state lives in the
    // daemon's registries, not in model-writable host files).
    for tool in [
        "new_cron_job",
        "list_cron_jobs",
        "new_trigger",
        "list_triggers",
    ] {
        assert!(
            names.contains(tool),
            "in-memory trigger/cron tool `{tool}` must remain registered; got {names:?}"
        );
    }
}

#[test]
fn subagent_tool_sets_omit_direct_os_tools_for_every_spec() {
    let resolver = tools::subagent_tool_sets(
        std::path::Path::new("/nonexistent-memory-dir").to_path_buf(),
        std::path::Path::new("/nonexistent-theway-base").to_path_buf(),
        harness_cell(),
        sandbox_exec(),
    );
    for spec in theway_daemon::agent_specs::spec_names() {
        let set = resolver(&spec);
        assert!(
            !set.is_empty(),
            "subagent spec `{spec}` tool set must not be empty in sandbox-only builds"
        );
        let names = names(&set);
        for tool in DIRECT_OS_LOCAL_TOOLS.iter().chain(DIRECT_OS_ENGINE_TOOLS) {
            assert!(
                !names.contains(*tool),
                "subagent spec `{spec}` must not get direct-OS tool `{tool}`; got {names:?}"
            );
        }
        for tool in EXECUTOR_BACKED_TOOLS {
            assert!(
                names.contains(*tool),
                "subagent spec `{spec}` keeps executor-backed `{tool}`; got {names:?}"
            );
        }
    }
}

/// Registration is only half of fail closed: an executor-backed call must also fail
/// explicitly (never hang, never touch the host) when it reaches the sandbox stub.
#[tokio::test]
async fn executor_backed_read_fails_closed_with_unsupported_kind() {
    let read = tools::read::ReadTool::new(sandbox_exec());
    let err = read
        .execute(
            "r-sandbox-gate",
            serde_json::json!({ "path": "/etc/hostname" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("sandbox stub must reject the read");
    let message = format!("{err}");
    assert!(
        message.contains("unsupported executor kind: sandbox"),
        "expected the explicit UnsupportedKind(Sandbox) failure, got: {message}"
    );
    // And nothing was read from the host: the error surfaced before any content.
}
