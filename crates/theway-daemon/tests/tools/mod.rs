//! Tests for the tool ASSEMBLY layer (`src/tools/mod.rs`) — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use theway_llm_provider::{Api, Model, ModelCost, Provider};

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

#[cfg(feature = "local")]
fn local_exec() -> Arc<dyn ToolExecutor> {
    Arc::new(crate::executor::local::LocalExecutor::new())
}

fn empty_skill_cell() -> SkillHarnessCell {
    Arc::new(SyncOnceCell::new())
}

fn names(tools: &[Arc<dyn AgentTool>]) -> Vec<String> {
    tools.iter().map(|t| t.definition().name.clone()).collect()
}

#[cfg(feature = "local")]
#[test]
fn local_tools_registers_all_local_only_bodies() {
    let tools = local_tools(local_exec());
    let names = names(&tools);

    for expected in LOCAL_ONLY_TOOL_NAMES {
        assert!(
            names.iter().any(|n| n == expected),
            "missing {expected} in {names:?}"
        );
    }
    assert!(names.iter().any(|n| n == "read"), "{names:?}");
    assert!(names.iter().any(|n| n == "write"), "{names:?}");
    assert!(names.iter().any(|n| n == "edit"), "{names:?}");
    assert!(names.iter().any(|n| n == "outline"), "{names:?}");
    assert!(names.iter().any(|n| n == "git"), "{names:?}");
    assert!(names.iter().any(|n| n == "web_fetch"), "{names:?}");
    assert!(names.iter().any(|n| n == "web_search"), "{names:?}");
}

#[cfg(feature = "local")]
#[test]
fn session_tool_set_assembles_local_engine_and_trigger_families() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Arc::new(DagEngine::new());
    let registry = AgentJobRegistry::new();
    let model = fake_model();
    let cell = empty_skill_cell();
    let session_id = "session-test";

    let tools = session_tool_set(
        dir.path(),
        dir.path(),
        &engine,
        &registry,
        &model,
        None,
        &cell,
        session_id,
        local_exec(),
    );

    let names = names(&tools);
    for expected in [
        "bash",
        "read",
        "write",
        "memory",
        "skill",
        "install_skill",
        "skill_builder",
        "subagent",
        "new_trigger",
        "list_triggers",
        "remove_trigger",
        "set_trigger_state",
        "new_cron_job",
        "list_cron_jobs",
        "remove_cron_job",
        "set_cron_job_state",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing {expected} in session tool set: {names:?}"
        );
    }
    // Engine tools are session-stamped at construction time; sanity-check that the
    // session-specific orchestration tools are the DAG family (name prefix dag_).
    assert!(
        names.iter().any(|n| n.starts_with("dag_")),
        "expected DAG tools in session tool set: {names:?}"
    );
}

#[cfg(feature = "local")]
#[test]
fn subagent_tool_and_node_launcher_build() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Arc::new(DagEngine::new());
    let registry = AgentJobRegistry::new();
    let model = fake_model();
    let cell = empty_skill_cell();

    let subagent = subagent_tool(
        model.clone(),
        None,
        registry.clone(),
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cell.clone(),
        Some("session-test".into()),
        local_exec(),
    );
    assert_eq!(subagent.definition().name, "subagent");

    let launcher = node_launcher(
        engine,
        model,
        None,
        std::env::current_dir().expect("cwd"),
        registry,
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        cell,
        local_exec(),
    );
    let cloned = Arc::clone(&launcher);
    assert_eq!(Arc::strong_count(&launcher), 2);
    drop(cloned);
}

#[cfg(feature = "local")]
#[test]
fn subagent_tool_sets_uses_kernel_assembly_resolver() {
    let dir = tempfile::tempdir().expect("tempdir");
    let resolver = subagent_tool_sets(
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        empty_skill_cell(),
        local_exec(),
    );

    // The resolver is keyed by spec name; asking for any spec should yield the
    // uniform set (engine tools minus subagent/dag_* plus local tools).
    let tools = resolver("explorer");
    let names = names(&tools);
    assert!(
        !names.iter().any(|n| n == "subagent" || n.starts_with("dag_")),
        "subagent tool set must not include orchestration tools: {names:?}"
    );
}
