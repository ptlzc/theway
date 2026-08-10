//! Tests for `skill_builder` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
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

fn build_test_harness(
    seed: Vec<Skill>,
) -> (Arc<AgentHarness>, SkillHarnessCell, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_path_buf();
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = seed;
    let dir_clone = dir_path.clone();
    let loader: ReloadSkillsFn = Arc::new(move || {
        let dir_for_fut = dir_clone.clone();
        Box::pin(async move {
            let env = theway_core::NativeEnv::new(
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            theway_core::load_skills(
                &env,
                &[dir_for_fut.to_string_lossy().as_ref()],
                CancellationToken::new(),
            )
            .await
        })
    });
    opts.reload_skills_fn = Some(loader);
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok(), "set once");
    (harness, cell, dir)
}

fn test_tool(cell: SkillHarnessCell, dir: &tempfile::TempDir) -> SkillBuilderTool {
    SkillBuilderTool::with_skills_root(cell, dir.path().to_path_buf())
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
        "description": "review rust code for unwrap abuse; use when reviewing rust PRs",
        "instructions": "1. grep for unwrap\n2. flag each in non-test code",
        "confirm": confirm,
    })
}

#[test]
fn render_produces_loadable_canonical_template() {
    let rendered = render_skill_md(
        "code-review-checklist",
        "review code; use when asked to review",
        "step one\nstep two",
        None,
    )
    .expect("render");
    assert!(rendered.starts_with("---\n"), "{rendered}");
    assert!(rendered.contains("# Code Review Checklist"), "{rendered}");
    assert!(rendered.contains("## Instructions"), "{rendered}");
    assert!(rendered.contains("step one\nstep two"), "{rendered}");
    assert!(
        !rendered.contains("## Examples"),
        "examples section must be omitted when not provided: {rendered}"
    );
    let parsed = parse_and_validate_skill_md(&rendered).expect("rendered skill must load");
    assert_eq!(parsed.name, "code-review-checklist");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

#[test]
fn render_includes_examples_section_when_provided() {
    let rendered = render_skill_md(
        "alpha",
        "desc; when",
        "body",
        Some("```\ntheway session export\n```"),
    )
    .expect("render");
    assert!(rendered.contains("## Examples"), "{rendered}");
    assert!(rendered.contains("theway session export"), "{rendered}");
}

#[test]
fn render_escapes_yaml_specials_and_folds_newlines_in_description() {
    let rendered = render_skill_md(
        "alpha",
        "tricky: contains #yaml \"specials\"\nand a second line",
        "body",
        None,
    )
    .expect("render");
    let parsed = parse_and_validate_skill_md(&rendered)
        .expect("description with yaml specials must stay loadable");
    assert_eq!(
        parsed.description,
        "tricky: contains #yaml \"specials\" and a second line"
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
}

/// Preview is a pure read — it must not consume a user confirmation. Only the
/// `confirm: true` write phase routes through the control-plane prompt, so the
/// "summarize recent work into a skill" flow costs the user exactly one approval.
#[test]
fn preview_is_allowed_and_only_confirm_prompts() {
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    let tool = SkillBuilderTool::with_skills_root(cell, PathBuf::from("/tmp"));

    let preview = tool.permission_classification(&build_args("alpha", false));
    assert!(
        matches!(preview, PermissionClassification::Allow),
        "preview must not prompt: {preview:?}"
    );

    let confirm = tool.permission_classification(&build_args("alpha", true));
    match confirm {
        PermissionClassification::Prompt { reason } => {
            assert!(reason.contains("alpha"), "{reason}");
        }
        other => panic!("confirm must prompt, got {other:?}"),
    }

    let bad_name = tool.permission_classification(&build_args("../etc", true));
    match bad_name {
        PermissionClassification::Prompt { reason } => {
            assert!(reason.contains("<invalid name>"), "{reason}");
        }
        other => panic!("confirm must prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn preview_returns_metadata_without_writing() {
    let (_harness, cell, dir) = build_test_harness(vec![]);
    let tool = test_tool(cell, &dir);

    let result = execute(&tool, build_args("alpha", false))
        .await
        .expect("preview should succeed");

    assert_eq!(result.details["phase"], "preview");
    assert_eq!(result.details["name"], "alpha");
    assert_eq!(result.details["existing"], false);
    assert_eq!(result.details["overwrite_required"], false);
    assert!(
        PathBuf::from(result.details["target_path"].as_str().unwrap())
            .ends_with(PathBuf::from("alpha").join("SKILL.md"))
    );
    assert!(
        !dir.path().join("alpha").exists(),
        "preview must not create any files"
    );
}

#[tokio::test]
async fn confirm_writes_skill_and_reloads_catalog() {
    let (harness, cell, dir) = build_test_harness(vec![]);
    let tool = test_tool(cell, &dir);

    let result = execute(&tool, build_args("alpha", true))
        .await
        .expect("build should succeed");

    assert_eq!(result.details["phase"], "installed");
    assert_eq!(result.details["installed_visible_in_catalog"], true);
    assert!(
        result.details["audit_entry_id"].as_str().is_some(),
        "audit entry must be recorded: {}",
        result.details
    );
    let on_disk = std::fs::read_to_string(dir.path().join("alpha/SKILL.md")).unwrap();
    assert!(on_disk.starts_with("---\nname: alpha\n"), "{on_disk}");
    assert!(
        harness.skills().iter().any(|s| s.name == "alpha"),
        "catalog must contain the new skill after reload"
    );
}

#[tokio::test]
async fn rejects_invalid_name_before_any_write() {
    let (_harness, cell, dir) = build_test_harness(vec![]);
    let tool = test_tool(cell, &dir);

    let err = execute(&tool, build_args("../escape", true))
        .await
        .expect_err("traversal name must be refused");
    let msg = format!("{err:?}");
    assert!(msg.contains("name"), "{msg}");
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "nothing may be written for an invalid name"
    );
}

#[tokio::test]
async fn overwrite_requires_explicit_flag() {
    let (_harness, cell, dir) = build_test_harness(vec![]);
    let tool = test_tool(cell, &dir);

    execute(&tool, build_args("alpha", true)).await.unwrap();

    // Same name, different instructions → must refuse without overwrite.
    let mut changed = build_args("alpha", true);
    changed["instructions"] = json!("totally different body");
    let err = execute(&tool, changed.clone())
        .await
        .expect_err("differing content must require overwrite");
    assert!(format!("{err:?}").contains("overwrite"), "{err:?}");

    changed["overwrite"] = json!(true);
    let result = execute(&tool, changed).await.expect("overwrite succeeds");
    assert_eq!(result.details["overwrote"], true);
    let on_disk = std::fs::read_to_string(dir.path().join("alpha/SKILL.md")).unwrap();
    assert!(on_disk.contains("totally different body"), "{on_disk}");
}

#[tokio::test]
async fn preview_warns_when_project_skill_shadows_new_name() {
    let project_skill = Skill {
        name: "alpha".into(),
        description: "project one".into(),
        file_path: "/proj/.theway/skills/alpha/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: SkillSource::Project,
    };
    let (_harness, cell, dir) = build_test_harness(vec![project_skill]);
    let tool = test_tool(cell, &dir);

    let result = execute(&tool, build_args("alpha", false)).await.unwrap();
    let warnings = result.details["warnings"].to_string();
    assert!(
        warnings.contains("project"),
        "must warn that a project skill shadows the new user skill: {warnings}"
    );
}
