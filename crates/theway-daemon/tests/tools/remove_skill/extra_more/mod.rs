//! Additional tests for `remove_skill`, kept in a separate bridged module so the
//! original mirrored suites stay untouched (see docs/rust-test-files.md).

use super::super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use theway_core::{
    AgentHarness, AgentHarnessOptions, LoadSkillsOutput, MemorySessionStorage, ReloadSkillsFn,
    Session, SessionStorage, Skill,
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

fn build(seed: Vec<Skill>, reload: Option<Vec<Skill>>) -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = seed;
    if let Some(reload_skills) = reload {
        let loader: ReloadSkillsFn = Arc::new(move || {
            let skills = reload_skills.clone();
            Box::pin(async move {
                LoadSkillsOutput {
                    skills,
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

fn empty_cell() -> SkillHarnessCell {
    Arc::new(SyncOnceCell::new())
}

fn tool_with(base_dir: PathBuf, cell: SkillHarnessCell) -> RemoveSkillTool {
    RemoveSkillTool::with_base_dir(cell, base_dir)
}

fn write_user_skill(base: &Path, name: &str) -> String {
    let dir = base.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("SKILL.md");
    std::fs::write(
        &path,
        format!("---\nname: {name}\ndescription: d\n---\nbody\n"),
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn deletion_target_rejects_skills_root_itself() {
    let root = Path::new("/home/u/.theway/skills");
    assert_eq!(deletion_target(root, root), None);
}

#[tokio::test]
async fn execute_missing_name_is_invalid_arguments_error() {
    let dir = tempfile::tempdir().unwrap();
    let tool = tool_with(dir.path().into(), empty_cell());

    let err = tool
        .execute("c1", serde_json::json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing name must fail during argument parsing");

    assert!(err.to_string().contains("invalid arguments"), "got: {err}");
}

#[tokio::test]
async fn execute_without_harness_cell_returns_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let tool = tool_with(dir.path().into(), empty_cell());

    let err = tool
        .execute(
            "c1",
            serde_json::json!({ "name": "foo", "confirm": true }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("uninitialized harness cell must fail");

    assert!(
        err.to_string().contains("not yet initialized"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_unknown_skill_suggests_closest_loaded_names() {
    let dir = tempfile::tempdir().unwrap();
    let (_harness, cell) = build(
        vec![
            skill("foobar", SkillSource::User, "/tmp/skills/foobar/SKILL.md"),
            skill("foo_baz", SkillSource::User, "/tmp/skills/foo_baz/SKILL.md"),
            skill("other", SkillSource::User, "/tmp/skills/other/SKILL.md"),
        ],
        None,
    );
    let tool = tool_with(dir.path().into(), cell);

    let err = tool
        .execute(
            "c1",
            serde_json::json!({ "name": "fo", "confirm": true }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("unknown skill must fail");

    let msg = err.to_string();
    assert!(msg.contains("no loaded skill named 'fo'"), "got: {msg}");
    assert!(msg.contains("Did you mean: foobar, foo_baz"), "got: {msg}");
}

#[tokio::test]
async fn execute_source_pin_user_previews_without_deleting() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo");
    let (_harness, cell) = build(vec![skill("foo", SkillSource::User, &fp)], None);
    let tool = tool_with(dir.path().into(), cell);

    let res = tool
        .execute(
            "c1",
            serde_json::json!({ "name": "foo", "source": "user" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("preview with a valid source pin must succeed");

    assert_eq!(res.details["phase"], "preview");
    assert_eq!(res.details["source"], "user");
    assert!(Path::new(&fp).exists(), "preview must not delete");
}

#[tokio::test]
async fn execute_maps_reload_failure_after_removal() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo");
    let (_harness, cell) = build(vec![skill("foo", SkillSource::User, &fp)], None);
    let tool = tool_with(dir.path().into(), cell);

    let err = tool
        .execute(
            "c1",
            serde_json::json!({ "name": "foo", "confirm": true }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("reload after remove must fail when no loader is configured");

    let msg = err.to_string();
    assert!(msg.contains("reload after remove:"), "got: {msg}");
    assert!(!Path::new(&fp).exists(), "file must be gone before reload");
}

#[tokio::test]
async fn execute_reports_still_present_when_reload_keeps_skill() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo");
    let seed = vec![skill("foo", SkillSource::User, &fp)];
    let (_harness, cell) = build(seed.clone(), Some(seed));
    let tool = tool_with(dir.path().into(), cell);

    let res = tool
        .execute(
            "c1",
            serde_json::json!({ "name": "foo", "confirm": true }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("remove itself succeeds");

    assert_eq!(res.details["phase"], "removed");
    assert_eq!(res.details["still_present_after_reload"], true);
    assert!(!Path::new(&fp).exists(), "file must still be deleted");
}
