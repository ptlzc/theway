//! Tests for `remove_skill` — split out of src (see docs/RUST_TEST_FILES.md).

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

/// Write a `<base>/skills/<name>/SKILL.md` on disk and return the absolute SKILL.md path.
async fn write_user_skill(base: &Path, name: &str) -> String {
    let dir = base.join("skills").join(name);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let content = format!("---\nname: {name}\ndescription: d\n---\nbody of {name}\n");
    let path = dir.join("SKILL.md");
    tokio::fs::write(&path, content).await.unwrap();
    path.to_string_lossy().to_string()
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

/// Harness whose `reload_skills_from_disk` re-scans `<base>/skills` from disk (so a removed
/// dir actually disappears from the reloaded catalog) and applies the overlay — mirroring
/// the real main.rs reload closure.
fn build(seed: Vec<Skill>, base: PathBuf) -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = seed;
    let base_for_reload = base.clone();
    let loader: ReloadSkillsFn = Arc::new(move || {
        let base = base_for_reload.clone();
        Box::pin(async move {
            let env = theway_daemon::env::native::NativeEnv::new(
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            let dir = base.join("skills");
            let mut out = theway_core::load_skills(
                &env,
                &[dir.to_string_lossy().as_ref()],
                CancellationToken::new(),
            )
            .await;
            for s in out.skills.iter_mut() {
                s.source = SkillSource::User;
            }
            let state = skill_overrides::load(&base).await;
            skill_overrides::apply(&state, &mut out.skills);
            out
        })
    });
    opts.reload_skills_fn = Some(loader);
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok());
    (harness, cell)
}

async fn exec(tool: &RemoveSkillTool, params: Value) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("c1", params, CancellationToken::new(), None)
        .await
}

#[test]
fn deletion_target_is_direct_child_of_skills_root() {
    let root = Path::new("/home/u/.theway/skills");
    // <name>/SKILL.md → remove the <name> dir.
    assert_eq!(
        deletion_target(root, Path::new("/home/u/.theway/skills/foo/SKILL.md")),
        Some(PathBuf::from("/home/u/.theway/skills/foo"))
    );
    // root-level <x>.md → remove the file.
    assert_eq!(
        deletion_target(root, Path::new("/home/u/.theway/skills/bar.md")),
        Some(PathBuf::from("/home/u/.theway/skills/bar.md"))
    );
    // outside the root → None (refused).
    assert_eq!(deletion_target(root, Path::new("/etc/passwd")), None);
}

#[tokio::test]
async fn preview_does_not_delete() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo").await;
    let (_h, cell) = build(
        vec![skill("foo", SkillSource::User, &fp)],
        dir.path().into(),
    );
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    let res = exec(&tool, json!({"name": "foo"}))
        .await
        .expect("preview ok");
    assert_eq!(res.details["phase"], "preview");
    assert_eq!(res.details["source"], "user");
    // File still on disk.
    assert!(Path::new(&fp).exists(), "preview must not delete");
}

#[tokio::test]
async fn confirm_deletes_and_reload_drops_it() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo").await;
    let (harness, cell) = build(
        vec![skill("foo", SkillSource::User, &fp)],
        dir.path().into(),
    );
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    let res = exec(&tool, json!({"name": "foo", "confirm": true}))
        .await
        .expect("remove ok");
    assert_eq!(res.details["phase"], "removed");
    assert_eq!(res.details["still_present_after_reload"], false);
    // Skill dir gone.
    assert!(!dir.path().join("skills").join("foo").exists());
    // Catalog no longer has it (reload re-scanned disk).
    assert!(
        !harness.skills().iter().any(|s| s.name == "foo"),
        "removed skill must not survive reload"
    );
}

#[tokio::test]
async fn builtin_cannot_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (_h, cell) = build(
        vec![skill("kp", SkillSource::Builtin, "<builtin>/kp/SKILL.md")],
        dir.path().into(),
    );
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());
    let err = exec(&tool, json!({"name": "kp", "confirm": true}))
        .await
        .expect_err("builtin remove rejected");
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(
        m.contains("builtin skill") && m.contains("disable"),
        "got: {m}"
    );
}

#[tokio::test]
async fn project_cannot_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (_h, cell) = build(
        vec![skill(
            "p",
            SkillSource::Project,
            "/repo/.theway/skills/p/SKILL.md",
        )],
        dir.path().into(),
    );
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());
    let err = exec(&tool, json!({"name": "p", "confirm": true}))
        .await
        .expect_err("project remove rejected");
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(
        m.contains("project skill") && m.contains("disable"),
        "got: {m}"
    );
}

#[tokio::test]
async fn remove_clears_overlay_entry() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo").await;
    // Pre-existing disabled overlay entry for foo.
    skill_overrides::set_and_save(dir.path(), "foo", SkillSource::User, false)
        .await
        .unwrap();
    let (_h, cell) = build(
        vec![skill("foo", SkillSource::User, &fp)],
        dir.path().into(),
    );
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    exec(&tool, json!({"name": "foo", "confirm": true}))
        .await
        .expect("remove ok");

    // Overlay no longer carries the stale entry.
    let state = skill_overrides::load(dir.path()).await;
    assert!(
        state.lookup("foo", SkillSource::User).is_none(),
        "remove must clear the overlay entry so reinstall starts fresh"
    );
}

#[tokio::test]
async fn writes_remove_audit() {
    let dir = tempfile::tempdir().unwrap();
    let fp = write_user_skill(dir.path(), "foo").await;
    let (harness, cell) = build(
        vec![skill("foo", SkillSource::User, &fp)],
        dir.path().into(),
    );
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());

    exec(&tool, json!({"name": "foo", "confirm": true}))
        .await
        .expect("remove ok");

    let entries = harness.session().entries().await.unwrap();
    let audit = entries.iter().find_map(|e| match e {
        theway_core::SessionTreeEntry::Custom {
            custom_type, data, ..
        } if custom_type == "skill_control_plane" => data.clone(),
        _ => None,
    });
    let data = audit.expect("skill_control_plane audit written");
    assert_eq!(data["op"], "remove");
    assert_eq!(data["name"], "foo");
    assert_eq!(data["source"], "user");
    let s = serde_json::to_string(&data).unwrap();
    assert!(
        !s.contains("body of foo"),
        "audit must not contain skill body: {s}"
    );
}

#[tokio::test]
async fn unknown_skill_is_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let (_h, cell) = build(vec![], dir.path().into());
    let tool = RemoveSkillTool::with_base_dir(cell, dir.path().into());
    let err = exec(&tool, json!({"name": "ghost", "confirm": true}))
        .await
        .expect_err("unknown skill errors");
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(m.contains("no loaded skill named 'ghost'"));
}
