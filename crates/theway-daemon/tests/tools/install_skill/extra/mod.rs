//! Additional tests for `install_skill`, kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::sync::Arc;
use theway_core::{
    AgentHarness, AgentHarnessOptions, LoadSkillsOutput, MemorySessionStorage, ReloadSkillsFn,
    Session, SessionStorage,
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

fn build_test_harness() -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    let loader: ReloadSkillsFn = Arc::new(move || {
        Box::pin(async move { LoadSkillsOutput {
            skills: vec![],
            diagnostics: vec![],
        } })
    });
    opts.reload_skills_fn = Some(loader);
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok(), "set once");
    (harness, cell)
}

fn empty_cell() -> SkillHarnessCell {
    Arc::new(SyncOnceCell::new())
}

async fn execute(
    tool: &InstallSkillTool,
    params: Value,
) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("call-1", params, CancellationToken::new(), None)
        .await
}

#[test]
fn default_skills_root_ends_with_skills_dir() {
    let root = default_skills_root();
    assert!(root.ends_with("skills"), "got: {root:?}");
}

#[test]
fn target_path_joins_skills_root_with_name() {
    let tool = InstallSkillTool::with_skills_root(empty_cell(), PathBuf::from("/tmp/base"));
    assert_eq!(
        tool.target_path("alpha"),
        PathBuf::from("/tmp/base").join("alpha").join("SKILL.md")
    );
}

#[test]
fn definition_label_and_execution_mode_are_registered() {
    let tool = InstallSkillTool::with_skills_root(empty_cell(), PathBuf::from("/tmp/base"));
    assert_eq!(tool.definition().name, "install_skill");
    assert_eq!(tool.label(), "install_skill");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Sequential));
}

#[tokio::test]
async fn execute_rejects_invalid_arguments() {
    let tool = InstallSkillTool::with_skills_root(empty_cell(), PathBuf::from("/tmp/base"));
    let err = execute(&tool, json!({ "source": "not-an-object" }))
        .await
        .expect_err("malformed arguments must fail");
    assert!(
        err.to_string().contains("invalid arguments"),
        "got: {err}"
    );
}

#[test]
fn audit_source_reference_path_and_content_are_bounded() {
    assert_eq!(
        audit_source_reference(&Source::Path {
            path: "/tmp/example/SKILL.md".into(),
        }),
        json!("/tmp/example/SKILL.md")
    );
    assert_eq!(
        audit_source_reference(&Source::Content {
            content: "secret body".into(),
        }),
        json!(null)
    );
}

#[test]
fn audit_url_reference_handles_unparseable_url() {
    let reference = audit_url_reference("not a url");
    assert_eq!(reference, json!({ "redacted": true }));
}

#[tokio::test]
async fn on_disk_skill_hash_nonexistent_and_non_utf8_are_none() {
    let dir = tempfile::tempdir().expect("tempdir");

    let missing = dir.path().join("missing").join("SKILL.md");
    assert_eq!(on_disk_skill_hash(&missing).await, None);

    let bin = dir.path().join("bin");
    tokio::fs::create_dir_all(&bin).await.unwrap();
    tokio::fs::write(bin.join("SKILL.md"), [0xff, 0xfe, 0x00]).await.unwrap();
    assert_eq!(on_disk_skill_hash(&bin.join("SKILL.md")).await, None);
}

#[tokio::test]
async fn installs_from_absolute_local_path() {
    let (harness, cell) = build_test_harness();
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("source-SKILL.md");
    tokio::fs::write(
        &source,
        "---\nname: path-skill\ndescription: from a local path\n---\nPath body.\n",
    )
    .await
    .unwrap();

    let tool = InstallSkillTool::with_skills_root(cell, dir.path().to_path_buf());

    // Preview from a local file must succeed and must not write the skill yet.
    let preview = execute(
        &tool,
        json!({ "source": { "type": "path", "path": source.to_string_lossy() } }),
    )
    .await
    .expect("path source preview should succeed");
    assert_eq!(preview.details["phase"], "preview");
    assert_eq!(preview.details["name"], "path-skill");
    assert!(
        !dir.path().join("path-skill").exists(),
        "preview must not write"
    );

    // Confirm writes the skill and hot-reloads (our harness reload returns an empty
    // catalog, so `installed_visible_in_catalog` is false but the install succeeds).
    let installed = execute(
        &tool,
        json!({
            "source": { "type": "path", "path": source.to_string_lossy() },
            "confirm": true,
        }),
    )
    .await
    .expect("path source install should succeed");
    assert_eq!(installed.details["phase"], "installed");
    assert_eq!(installed.details["name"], "path-skill");

    let written = tokio::fs::read_to_string(dir.path().join("path-skill").join("SKILL.md"))
        .await
        .expect("SKILL.md written");
    assert!(written.contains("Path body."), "{written}");

    // The persistent audit entry records the redacted local path.
    let entries = harness.session().entries().await.expect("read entries");
    let custom = entries.iter().find_map(|e| match e {
        theway_core::SessionTreeEntry::Custom {
            custom_type, data, ..
        } if custom_type == "skill_install" => data.clone(),
        _ => None,
    });
    let data = custom.expect("audit entry");
    assert_eq!(data["source_kind"], "path");
    assert_eq!(data["source"], json!(source.to_string_lossy()));
}

#[tokio::test]
async fn execute_rejects_relative_path_source() {
    let tool = InstallSkillTool::with_skills_root(empty_cell(), PathBuf::from("/tmp/base"));
    let err = execute(
        &tool,
        json!({ "source": { "type": "path", "path": "relative/SKILL.md" } }),
    )
    .await
    .expect_err("relative path source must fail");
    assert!(
        err.to_string().contains("path must be absolute"),
        "got: {err}"
    );
}

#[test]
fn audit_source_reference_url_uses_redacted_url_reference() {
    let reference = audit_source_reference(&Source::Url {
        url: "https://example.com/skill.md".into(),
    });
    assert_eq!(reference["scheme"], "https");
    assert_eq!(reference["host"], "example.com");
    assert_eq!(reference["redacted"], true);
    let serialized = serde_json::to_string(&reference).unwrap();
    assert!(
        !serialized.contains("skill.md"),
        "url path must be redacted: {serialized}"
    );
}

#[test]
fn default_skills_root_and_target_path_are_stable() {
    let root = default_skills_root();
    assert!(root.ends_with("skills"), "got: {root:?}");
}

#[tokio::test]
async fn on_disk_skill_hash_matches_content_hash_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("name").join("SKILL.md");
    tokio::fs::create_dir_all(target.parent().unwrap()).await.unwrap();
    tokio::fs::write(&target, "---\r\nname: name\r\ndescription: d\r\n---\r\nbody\r\n")
        .await
        .unwrap();
    let hash = on_disk_skill_hash(&target).await.expect("utf8 file");
    assert_eq!(hash.len(), 64);
}

#[tokio::test]
async fn install_includes_reload_diagnostics_matching_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dir_path = dir.path().to_path_buf();
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    let loader: ReloadSkillsFn = Arc::new(move || {
        let dir_for_fut = dir_path.clone();
        Box::pin(async move {
            let target = dir_for_fut
                .join("alpha")
                .join("SKILL.md")
                .to_string_lossy()
                .to_string();
            LoadSkillsOutput {
                skills: vec![],
                diagnostics: vec![
                    theway_core::SkillDiagnostic {
                        code: theway_core::SkillDiagnosticCode::ReadFailed,
                        message: "read failed".into(),
                        path: target.clone(),
                    },
                    theway_core::SkillDiagnostic {
                        code: theway_core::SkillDiagnosticCode::ParseFailed,
                        message: "parse failed".into(),
                        path: "other/skill.md".into(),
                    },
                ],
            }
        })
    });
    opts.reload_skills_fn = Some(loader);
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok(), "set once");

    let tool = InstallSkillTool::with_skills_root(cell, dir.path().to_path_buf());
    let md = "---\nname: alpha\ndescription: d\n---\nbody\n";
    let result = execute(
        &tool,
        json!({ "source": { "type": "content", "content": md }, "confirm": true }),
    )
    .await
    .expect("install succeeds with reload diagnostics");
    let warnings = result.details["warnings"].to_string();
    assert!(warnings.contains("read failed"), "got: {warnings}");
    assert!(!warnings.contains("parse failed"), "unrelated diagnostic leaked: {warnings}");
}

#[tokio::test]
async fn atomic_write_skill_reports_rename_failure() {
    let dir = tempfile::tempdir().unwrap();
    // A directory at the target path makes the final rename fail on Unix (and is
    // also an invalid skill target), exercising the error branch.
    let target = dir.path().join("occupied");
    tokio::fs::create_dir(&target).await.unwrap();

    let err = atomic_write_skill(&target, "---\nname: x\ndescription: d\n---\nbody\n")
        .await
        .expect_err("rename over a directory must fail");
    assert!(err.to_string().contains("rename"), "got: {err}");
}
