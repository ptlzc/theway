//! Tests for `set_skill_state` — split out of src (see docs/rust-test-files.md).

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

/// Build a harness whose catalog is `seed` and whose `reload_skills_from_disk` re-derives
/// the catalog from `seed` with the overlay at `base_dir` applied — mirroring how the real
/// main.rs reload closure layers the overlay on top of the loaded skills.
fn build(seed: Vec<Skill>, base_dir: PathBuf) -> (Arc<AgentHarness>, SkillHarnessCell) {
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(fake_model(), session);
    opts.skills = seed.clone();
    let seed_for_reload = seed.clone();
    let base_for_reload = base_dir.clone();
    let loader: ReloadSkillsFn = Arc::new(move || {
        let mut skills = seed_for_reload.clone();
        let base = base_for_reload.clone();
        Box::pin(async move {
            let state = skill_overrides::load(&base).await;
            skill_overrides::apply(&state, &mut skills);
            theway_core::LoadSkillsOutput {
                skills,
                diagnostics: vec![],
            }
        })
    });
    opts.reload_skills_fn = Some(loader);
    let harness = Arc::new(AgentHarness::new(opts));
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    assert!(cell.set(harness.clone()).is_ok());
    (harness, cell)
}

async fn exec(tool: &SetSkillStateTool, params: Value) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("c1", params, CancellationToken::new(), None)
        .await
}

#[tokio::test]
async fn preview_does_not_write_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let (_h, cell) = build(
        vec![skill("foo", SkillSource::User, false)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    let res = exec(&tool, json!({"name": "foo", "enabled": false}))
        .await
        .expect("preview ok");
    assert_eq!(res.details["phase"], "preview");
    assert_eq!(res.details["currently_enabled"], true);
    assert_eq!(res.details["target_enabled"], false);
    // No overlay file written.
    assert!(!skill_overrides::state_path(dir.path()).exists());
}

#[tokio::test]
async fn disable_then_reload_reflects_state() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, cell) = build(
        vec![skill("foo", SkillSource::User, false)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    let res = exec(
        &tool,
        json!({"name": "foo", "enabled": false, "confirm": true}),
    )
    .await
    .expect("apply ok");
    assert_eq!(res.details["phase"], "applied");
    assert_eq!(res.details["enabled"], false);
    assert_eq!(res.details["effective_enabled_after_reload"], false);
    // Overlay persisted.
    let state = skill_overrides::load(dir.path()).await;
    assert_eq!(
        state.lookup("foo", SkillSource::User).map(|e| e.enabled),
        Some(false)
    );
    // Harness catalog now shows the skill disabled.
    let foo = harness
        .skills()
        .into_iter()
        .find(|s| s.name == "foo")
        .unwrap();
    assert!(foo.disable_model_invocation);
}

#[tokio::test]
async fn classifier_routes_disable_through_allow_and_enable_through_prompt() {
    // Issue #110 sub-PR 3 (Tools-MCP): the old PR #108 hard-block on `enabled: true` is
    // gone. The classifier now narrows disables to `Allow` and routes enables through the
    // `on_control_plane_prompt` channel so a real user (not the model) approves each
    // re-enable. This test asserts the per-arg classification shape; the integration test
    // for "Prompt + Deny actually blocks the tool" lives in the agent crate's
    // `permission_classification_prompt_with_hook_deny_blocks_and_emits_audit_event`.
    let dir = tempfile::tempdir().unwrap();
    let (_harness, cell) = build(
        vec![skill("foo", SkillSource::User, true)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    // Disable = narrowing = Allow.
    let disable = tool.permission_classification(&json!({"name": "foo", "enabled": false}));
    assert!(
        matches!(disable, PermissionClassification::Allow),
        "disable must classify as Allow (narrowing), got {disable:?}"
    );

    // Enable = escalating = Prompt with the skill name in the reason. The bounded reason
    // is what the embedder renders on the confirmation card (per §6b.5 prompt card UX).
    let enable = tool.permission_classification(&json!({"name": "foo", "enabled": true}));
    match enable {
        PermissionClassification::Prompt { reason } => {
            assert!(
                reason.contains("re-enable"),
                "reason must signal escalation, got: {reason}"
            );
            assert!(
                reason.contains("`foo`"),
                "reason must include the bounded skill name, got: {reason}"
            );
        }
        other => panic!("enable must classify as Prompt, got {other:?}"),
    }

    // Missing `enabled` field defaults to `false` (narrowing) — defensive default.
    let missing = tool.permission_classification(&json!({"name": "foo"}));
    assert!(
        matches!(missing, PermissionClassification::Allow),
        "missing enabled must default to narrowing (Allow), got {missing:?}"
    );
}

#[tokio::test]
async fn enable_no_longer_short_circuits_in_execute() {
    // Regression for PR #108's lifted hard-block: `enabled: true` is no longer rejected
    // at execute() entry. (The runtime gate is `permission_classification` + the
    // embedder's prompt hook; if the user denies, the agent loop never calls execute. If
    // the user accepts, execute proceeds and the skill is re-enabled.)
    let dir = tempfile::tempdir().unwrap();
    let (harness, cell) = build(
        vec![skill("foo", SkillSource::User, true)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    // Direct execute() of an enable + confirm now succeeds (no model-side reject).
    let result = exec(
        &tool,
        json!({"name": "foo", "enabled": true, "confirm": true}),
    )
    .await
    .expect("execute(enable) must succeed once the prompt-gate stopgap is lifted");
    assert!(
        !result.content.is_empty(),
        "successful enable returns a result message"
    );

    // Skill is now enabled in the live catalog after the reload.
    let foo = harness
        .skills()
        .into_iter()
        .find(|s| s.name == "foo")
        .unwrap();
    assert!(
        !foo.disable_model_invocation,
        "skill is enabled after execute(enable)"
    );
}

#[tokio::test]
async fn writes_skill_control_plane_audit() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, cell) = build(
        vec![skill("foo", SkillSource::User, false)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());

    exec(
        &tool,
        json!({"name": "foo", "enabled": false, "confirm": true}),
    )
    .await
    .expect("apply ok");

    let entries = harness.session().entries().await.unwrap();
    let audit = entries.iter().find_map(|e| match e {
        theway_core::SessionTreeEntry::Custom {
            custom_type, data, ..
        } if custom_type == "skill_control_plane" => data.clone(),
        _ => None,
    });
    let data = audit.expect("skill_control_plane audit written");
    assert_eq!(data["op"], "set_state");
    assert_eq!(data["name"], "foo");
    assert_eq!(data["source"], "user");
    assert_eq!(data["before_enabled"], true);
    assert_eq!(data["after_enabled"], false);
    // No body leak.
    let s = serde_json::to_string(&data).unwrap();
    assert!(
        !s.contains("body"),
        "audit must not contain skill body: {s}"
    );
}

#[tokio::test]
async fn unknown_skill_is_typed_error_with_hint() {
    let dir = tempfile::tempdir().unwrap();
    let (_h, cell) = build(
        vec![skill("formatter", SkillSource::User, false)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());
    let err = exec(&tool, json!({"name": "format", "enabled": false}))
        .await
        .expect_err("unknown skill errors");
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(m.contains("no loaded skill named 'format'"));
}

#[tokio::test]
async fn mismatched_source_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    // Active foo is User; caller pins project → reject.
    let (_h, cell) = build(
        vec![skill("foo", SkillSource::User, false)],
        dir.path().into(),
    );
    let tool = SetSkillStateTool::with_base_dir(cell, dir.path().into());
    let err = exec(
        &tool,
        json!({"name": "foo", "source": "project", "enabled": false}),
    )
    .await
    .expect_err("mismatched source errors");
    let AgentToolError::Message(m) = err else {
        panic!("typed error")
    };
    assert!(m.contains("active from source 'user'"), "got: {m}");
}
