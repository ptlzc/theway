//! Additional tests for `skill_builder`, kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::sync::Arc;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, Skill,
    SkillSource,
};
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

fn empty_cell() -> SkillHarnessCell {
    Arc::new(SyncOnceCell::new())
}

fn build_test_harness(seed: Vec<Skill>) -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = seed;
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok(), "set once");
    (harness, cell)
}

async fn execute(
    tool: &SkillBuilderTool,
    params: Value,
) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("call-1", params, CancellationToken::new(), None)
        .await
}

fn build_args(name: &str, confirm: bool) -> Value {
    json!({
        "name": name,
        "description": "review rust code for unwrap abuse",
        "instructions": "1. grep for unwrap\n2. flag each in non-test code",
        "confirm": confirm,
    })
}

#[test]
fn render_rejects_empty_description() {
    let err = render_skill_md("alpha", "   \n\t", "body", None).expect_err("empty description");
    assert!(
        err.to_string().contains("description must not be empty"),
        "got: {err}"
    );
}

#[test]
fn render_rejects_empty_instructions() {
    let err =
        render_skill_md("alpha", "desc", "   \n\t", None).expect_err("empty instructions");
    assert!(
        err.to_string().contains("instructions must not be empty"),
        "got: {err}"
    );
}

#[test]
fn title_from_name_capitalizes_and_filters_empty_segments() {
    assert_eq!(title_from_name("alpha"), "Alpha");
    assert_eq!(
        title_from_name("code-review-checklist"),
        "Code Review Checklist"
    );
    assert_eq!(title_from_name("-leading-trailing-"), "Leading Trailing");
    assert_eq!(title_from_name(""), "");
}

#[test]
fn definition_label_and_execution_mode_are_registered() {
    let tool = SkillBuilderTool::with_skills_root(empty_cell(), PathBuf::from("/tmp/base"));
    assert_eq!(tool.definition().name, "skill_builder");
    assert_eq!(tool.label(), "skill_builder");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Sequential));
}

#[tokio::test]
async fn execute_rejects_invalid_arguments() {
    let tool = SkillBuilderTool::with_skills_root(empty_cell(), PathBuf::from("/tmp/base"));
    let err = execute(&tool, json!({ "name": "missing-fields" }))
        .await
        .expect_err("missing description/instructions must fail");
    assert!(
        err.to_string().contains("invalid arguments"),
        "got: {err}"
    );
}

#[tokio::test]
async fn preview_warns_when_builtin_skill_is_shadowed() {
    let builtin = Skill {
        name: "alpha".into(),
        description: "builtin alpha".into(),
        file_path: "/builtin/alpha/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: SkillSource::Builtin,
    };
    let (_harness, cell) = build_test_harness(vec![builtin]);
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = SkillBuilderTool::with_skills_root(cell, dir.path().to_path_buf());

    let preview = execute(&tool, build_args("alpha", false))
        .await
        .expect("preview should succeed");
    let warnings = preview.details["warnings"].to_string();
    assert!(
        warnings.contains("shadow the builtin skill 'alpha'"),
        "expected builtin shadow warning, got: {warnings}"
    );
}

#[tokio::test]
async fn confirm_without_harness_writes_skill_then_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = SkillBuilderTool::with_skills_root(empty_cell(), dir.path().to_path_buf());

    let err = execute(&tool, build_args("orphan", true))
        .await
        .expect_err("missing harness must fail");
    assert!(
        err.to_string().contains("skill_builder not yet initialized"),
        "got: {err}"
    );

    // The atomic write happens before the harness is resolved; a failed harness lookup
    // must not leave a partial file behind.
    let written = tokio::fs::read_to_string(dir.path().join("orphan").join("SKILL.md"))
        .await
        .expect("SKILL.md should be written before the harness lookup fails");
    assert!(written.contains("name: orphan"), "{written}");
}
