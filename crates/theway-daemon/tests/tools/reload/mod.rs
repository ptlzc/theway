//! Tests for the `reload` tool (issue #50) — split out of src (see docs/rust-test-files.md).

use super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::sync::Arc;
use theway_core::{
    AgentHarness, AgentHarnessOptions, LoadSkillsOutput, MemorySessionStorage, ReloadSkillsFn,
    Session, SessionStorage, Skill, SkillSource,
};
use theway_llm_provider::{Api, Model, ModelCost, Provider};

use crate::commands::Registry;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;

fn fake_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
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

fn skill(name: &str, source: SkillSource) -> Skill {
    Skill {
        name: name.into(),
        description: "d".into(),
        file_path: format!("/tmp/{name}/SKILL.md"),
        content: "body".into(),
        disable_model_invocation: false,
        source,
    }
}

/// Harness whose reload closure swaps the catalog for `["reloaded"]` when
/// `with_reload` is set; without it, reload fails (`NotConfigured`).
pub(super) fn build_harness(with_reload: bool) -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = vec![skill("seed", SkillSource::User)];
    if with_reload {
        let loader: ReloadSkillsFn = Arc::new(|| {
            Box::pin(async move {
                LoadSkillsOutput {
                    skills: vec![skill("reloaded", SkillSource::User)],
                    diagnostics: vec![],
                }
            })
        });
        opts.reload_skills_fn = Some(loader);
    }
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok());
    (harness, cell)
}

/// Pin a hermetic runtime (registry + cwd + trigger executor + fresh
/// revision) so tests never race on the process-global installed by
/// `TurnHost::new`.
fn runtime_for(
    harness: &Arc<AgentHarness>,
    cwd: &std::path::Path,
    registry: Arc<Registry>,
) -> Arc<ReloadRuntime> {
    let trigger_executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    Arc::new(ReloadRuntime {
        registry,
        cwd: cwd.to_path_buf(),
        trigger_executor,
        revision: Arc::new(AtomicU64::new(0)),
    })
}

#[tokio::test]
async fn execute_rescans_file_commands_and_skills_then_bumps_revision() {
    // Arrange: temp cwd with one claude-code-format file command; harness
    // whose reload closure swaps in the "reloaded" skill; pinned runtime.
    let dir = tempfile::tempdir().unwrap();
    let commands_dir = dir.path().join(".agents").join("commands");
    std::fs::create_dir_all(&commands_dir).unwrap();
    std::fs::write(
        commands_dir.join("lint.md"),
        "---\ndescription: run lint\n---\nrun the linter with $ARGUMENTS\n",
    )
    .unwrap();
    let (harness, cell) = build_harness(true);
    let registry = Arc::new(Registry::with_daemon_commands());
    let runtime = runtime_for(&harness, dir.path(), registry.clone());
    let tool = ReloadTool::with_runtime(cell, runtime.clone());

    // Act
    let result = tool
        .execute("c1", serde_json::json!({}), CancellationToken::new(), None)
        .await;

    // Assert: rescan landed, skill catalog reloaded, revision bumped and
    // reported back to the LLM.
    let ok = result.unwrap_or_else(|e| panic!("reload should succeed: {e}"));
    assert_eq!(runtime.revision.load(Ordering::SeqCst), 1);
    assert!(harness.skills().iter().any(|s| s.name == "reloaded"));
    assert!(
        registry.file_command_names().contains(&"/lint".to_string()),
        "rescanned file commands: {:?}",
        registry.file_command_names()
    );
    assert_eq!(ok.details["runtime_revision"], serde_json::json!(1));
}

#[tokio::test]
async fn execute_failure_does_not_bump_revision() {
    // Arrange: harness without a reload loader → `reload_everything` errors.
    let dir = tempfile::tempdir().unwrap();
    let (harness, cell) = build_harness(false);
    let registry = Arc::new(Registry::with_daemon_commands());
    let runtime = runtime_for(&harness, dir.path(), registry);
    let tool = ReloadTool::with_runtime(cell, runtime.clone());

    // Act
    let result = tool
        .execute("c1", serde_json::json!({}), CancellationToken::new(), None)
        .await;

    // Assert: revision untouched, error surfaced.
    assert!(result.is_err());
    assert_eq!(runtime.revision.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_without_harness_cell_returns_error_not_panic() {
    // Arrange: the harness cell is intentionally never set.
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    let tool = ReloadTool::new(cell);

    // Act
    let result = tool
        .execute("c1", serde_json::json!({}), CancellationToken::new(), None)
        .await;

    // Assert: typed error, no panic.
    assert!(matches!(
        result,
        Err(AgentToolError::Message(ref m)) if m.contains("harness")
    ));
}

#[test]
fn definition_is_snake_case_with_effect_hint() {
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    let tool = ReloadTool::new(cell);

    assert_eq!(tool.definition().name, "reload");
    assert_eq!(tool.label(), "reload");
    assert!(tool.definition().description.contains("installing a skill"));
}


#[test]
fn install_runtime_publishes_process_global_runtime() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (harness, _cell) = build_harness(true);
    let registry = Arc::new(Registry::with_daemon_commands());
    let runtime = runtime_for(&harness, dir.path(), registry.clone());

    // Act
    let _ = install_runtime(ReloadRuntime {
        registry: runtime.registry.clone(),
        cwd: runtime.cwd.clone(),
        trigger_executor: runtime.trigger_executor.clone(),
        revision: runtime.revision.clone(),
    });

    // Assert: the installed handle is published (last-write-wins). Another
    // daemon test may concurrently install a different runtime, so only assert
    // that the global is populated after installation.
    assert!(current_runtime().is_some());
}

#[tokio::test]
async fn execute_without_pinned_runtime_uses_installed_global_runtime() {
    // Arrange: temp cwd with a file command; harness reload closure that swaps
    // in the "reloaded" skill; the runtime is installed globally instead of
    // pinned on the tool.
    let dir = tempfile::tempdir().unwrap();
    let commands_dir = dir.path().join(".agents").join("commands");
    std::fs::create_dir_all(&commands_dir).unwrap();
    std::fs::write(
        commands_dir.join("lint.md"),
        "---\ndescription: run lint\n---\nrun the linter with $ARGUMENTS\n",
    )
    .unwrap();
    let (harness, cell) = build_harness(true);
    let registry = Arc::new(Registry::with_daemon_commands());
    let runtime = runtime_for(&harness, dir.path(), registry.clone());
    let _ = install_runtime(ReloadRuntime {
        registry: runtime.registry.clone(),
        cwd: runtime.cwd.clone(),
        trigger_executor: runtime.trigger_executor.clone(),
        revision: runtime.revision.clone(),
    });
    let tool = ReloadTool::new(cell);

    // Act
    let result = tool
        .execute("c1", serde_json::json!({}), CancellationToken::new(), None)
        .await;

    // Assert: the global runtime path succeeds. (Another test may overwrite the
    // global, but every daemon runtime in this target handles `/reload` the same.)
    let ok = result.unwrap_or_else(|e| panic!("reload should succeed: {e}"));
    assert!(ok.details["runtime_revision"].is_u64());
}

#[test]
fn execution_mode_is_sequential() {
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    let tool = ReloadTool::new(cell);

    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Sequential));
}
