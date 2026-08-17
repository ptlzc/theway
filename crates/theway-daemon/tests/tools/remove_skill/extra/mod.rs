//! Additional tests for `remove_skill`, kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::path::{Path, PathBuf};
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

fn skill(name: &str, source: SkillSource, file_path: &str) -> Skill {
    Skill {
        name: name.into(),
        description: "d".into(),
        file_path: file_path.into(),
        content: "body".into(),
        disable_model_invocation: false,
        source,
    }
}

/// Harness whose catalog is `seed`; reload always returns an empty catalog, which is
/// enough for the extra branches under test (file deletion, missing-file idempotency,
/// source-pin errors, classifier/definition metadata).
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

async fn exec(tool: &RemoveSkillTool, params: Value) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("c1", params, CancellationToken::new(), None)
        .await
}

#[test]
fn deletion_target_rejects_relative_escape_paths() {
    let root = Path::new("/home/u/.theway/skills");
    assert_eq!(
        deletion_target(root, Path::new("/home/u/.theway/skills/../other/SKILL.md")),
        None
    );
    assert_eq!(deletion_target(root, Path::new("relative/SKILL.md")), None);
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
fn permission_classification_returns_prompt_with_bounded_name() {
    let tool = RemoveSkillTool::with_base_dir(empty_cell(), PathBuf::from("/tmp"));

    match tool.permission_classification(&json!({"name": "foo"})) {
        PermissionClassification::Prompt { reason } => {
            assert!(
                reason.contains("remove user skill `foo`"),
                "got: {reason}"
            );
        }
        other => panic!("remove must classify as Prompt, got {other:?}"),
    }

    let missing = tool.permission_classification(&json!({}));
    assert!(
        matches!(missing, PermissionClassification::Prompt { reason } if reason.contains("<unknown>")),
        "missing name must use the bounded <unknown> placeholder"
    );
}

#[test]
fn definition_label_and_execution_mode_are_registered() {
    let tool = RemoveSkillTool::with_base_dir(empty_cell(), PathBuf::from("/tmp"));

    assert_eq!(tool.definition().name, "remove_skill");
    assert_eq!(tool.label(), "remove_skill");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Sequential));
}

#[tokio::test]
async fn execute_removes_root_level_skill_file() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let skills_root = dir.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();
    let file_path = skills_root.join("standalone.md");
    std::fs::write(&file_path, "---\nname: standalone\ndescription: d\n---\nbody").unwrap();
    let (_harness, cell) = build(vec![skill(
        "standalone",
        SkillSource::User,
        &file_path.to_string_lossy(),
    )]);
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    // Act
    let res = exec(&tool, json!({"name": "standalone", "confirm": true}))
        .await
        .expect("root-level file removal succeeds");

    // Assert
    assert_eq!(res.details["phase"], "removed");
    assert_eq!(res.details["still_present_after_reload"], false);
    assert!(!file_path.exists(), "root-level skill file must be deleted");
}

#[tokio::test]
async fn execute_missing_file_is_idempotent_success() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("skills").join("ghost").join("SKILL.md");
    let (_harness, cell) = build(vec![skill(
        "ghost",
        SkillSource::User,
        &file_path.to_string_lossy(),
    )]);
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    // Act
    let res = exec(&tool, json!({"name": "ghost", "confirm": true}))
        .await
        .expect("already-missing skill is removed idempotently");

    // Assert
    assert_eq!(res.details["phase"], "removed");
    assert_eq!(res.details["still_present_after_reload"], false);
}

#[tokio::test]
async fn execute_rejects_source_pin_mismatch() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("skills").join("foo").join("SKILL.md");
    let (_harness, cell) = build(vec![skill(
        "foo",
        SkillSource::User,
        &file_path.to_string_lossy(),
    )]);
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    // Act
    let err = exec(
        &tool,
        json!({"name": "foo", "source": "builtin", "confirm": true}),
    )
    .await
    .expect_err("builtin source pin on a user skill must fail");

    // Assert
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(
        m.contains("only user-installed skills can be removed"),
        "got: {m}"
    );
}

#[tokio::test]
async fn execute_rejects_invalid_source() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("skills").join("foo").join("SKILL.md");
    let (_harness, cell) = build(vec![skill(
        "foo",
        SkillSource::User,
        &file_path.to_string_lossy(),
    )]);
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    // Act
    let err = exec(
        &tool,
        json!({"name": "foo", "source": "bogus", "confirm": true}),
    )
    .await
    .expect_err("invalid source must fail");

    // Assert
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(m.contains("invalid `source`"), "got: {m}");
}
