//! Additional tests for `set_skill_state`, kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::path::PathBuf;
use std::sync::Arc;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, ReloadSkillsFn, Session,
    SessionStorage, Skill,
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

fn skill(name: &str, source: SkillSource, disabled: bool) -> Skill {
    Skill {
        name: name.into(),
        description: "d".into(),
        file_path: format!("/tmp/{name}/SKILL.md"),
        content: "body".into(),
        disable_model_invocation: disabled,
        source,
    }
}

/// Harness whose reload returns an empty catalog. Good enough for the extra branches
/// here (preview no-change, case-insensitive source match, invalid source, metadata).
fn build(seed: Vec<Skill>) -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = seed;
    let loader: ReloadSkillsFn = Arc::new(move || {
        Box::pin(async move { theway_core::LoadSkillsOutput {
            skills: vec![],
            diagnostics: vec![],
        } })
    });
    opts.reload_skills_fn = Some(loader);
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok());
    (harness, cell)
}

fn empty_cell() -> SkillHarnessCell {
    Arc::new(SyncOnceCell::new())
}

async fn exec(tool: &SetSkillStateTool, params: Value) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("c1", params, CancellationToken::new(), None)
        .await
}

#[test]
fn parse_source_is_case_insensitive_and_rejects_unknown() {
    assert_eq!(parse_source("USER").unwrap(), SkillSource::User);
    assert_eq!(parse_source("builtin").unwrap(), SkillSource::Builtin);
    assert_eq!(parse_source("Project").unwrap(), SkillSource::Project);

    let err = parse_source("bogus").expect_err("unknown source errors");
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(m.contains("invalid `source`"), "got: {m}");
}

#[test]
fn enabled_word_returns_static_labels() {
    assert_eq!(enabled_word(true), "enabled");
    assert_eq!(enabled_word(false), "disabled");
}

#[test]
fn permission_classification_missing_name_uses_unknown_placeholder() {
    let tool = SetSkillStateTool::with_base_dir(empty_cell(), PathBuf::from("/tmp"));

    let classification = tool.permission_classification(&json!({"enabled": true}));
    assert!(
        matches!(classification, PermissionClassification::Prompt { reason } if reason.contains("<unknown>")),
        "missing name must use the bounded <unknown> placeholder"
    );
}

#[test]
fn definition_label_and_execution_mode_are_registered() {
    let tool = SetSkillStateTool::with_base_dir(empty_cell(), PathBuf::from("/tmp"));

    assert_eq!(tool.definition().name, "set_skill_state");
    assert_eq!(tool.label(), "set_skill_state");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Sequential));
}

#[tokio::test]
async fn preview_no_change_flags_noop_and_does_not_write_overlay() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (_harness, cell) = build(vec![skill("foo", SkillSource::User, false)]);
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    // Act
    let res = exec(&tool, json!({"name": "foo", "enabled": true}))
        .await
        .expect("preview ok");

    // Assert
    assert_eq!(res.details["phase"], "preview");
    assert_eq!(res.details["no_change"], true);
    let content = serde_json::to_string(&res.content).unwrap();
    assert!(content.contains("(no change)"), "got: {content}");
    assert!(!skill_overrides::state_path(dir.path()).exists());
}

#[tokio::test]
async fn execute_accepts_case_insensitive_source_match() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (_harness, cell) = build(vec![skill("foo", SkillSource::User, true)]);
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    // Act
    let res = exec(
        &tool,
        json!({"name": "foo", "source": "USER", "enabled": true, "confirm": true}),
    )
    .await
    .expect("case-insensitive source match succeeds");

    // Assert
    assert_eq!(res.details["phase"], "applied");
    assert_eq!(res.details["source"], "user");
}

#[tokio::test]
async fn execute_rejects_invalid_source() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let (_harness, cell) = build(vec![skill("foo", SkillSource::User, false)]);
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    // Act
    let err = exec(
        &tool,
        json!({"name": "foo", "source": "bogus", "enabled": false}),
    )
    .await
    .expect_err("invalid source must fail");

    // Assert
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(m.contains("invalid `source`"), "got: {m}");
}
