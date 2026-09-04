//! Tests for `commands::skills` — split out of src (see docs/rust-test-files.md).

use std::path::Path;
use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentToolResult, LoadSkillsOutput, MemorySessionStorage,
    ReloadSkillsFn, Session, SessionStorage, Skill, SkillSource,
};
use theway_llm_provider::{Api, Model, Provider, UserContentBlock};
use theway_transport::commands::{CommandCtx, CommandOutcome};

use super::*;
use crate::commands::DaemonCtx;
use crate::test_env::{EnvGuard, ENV_LOCK};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::runtime_storage::local_runtime_storage;

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn new_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn sample_skill(name: &str, source: SkillSource) -> Skill {
    Skill {
        name: name.into(),
        description: format!("{name} description"),
        file_path: format!("/tmp/{name}/SKILL.md"),
        content: "body".into(),
        disable_model_invocation: false,
        source,
    }
}

fn harness_with_skills(session: Session, skills: Vec<Skill>) -> Arc<AgentHarness> {
    let options = AgentHarnessOptions {
        skills,
        ..AgentHarnessOptions::new(faux_model(), session)
    };
    Arc::new(AgentHarness::new(options))
}

fn harness_with_reload_fn(
    session: Session,
    skills: Vec<Skill>,
    reload_skills_fn: Option<ReloadSkillsFn>,
) -> Arc<AgentHarness> {
    let options = AgentHarnessOptions {
        skills,
        reload_skills_fn,
        ..AgentHarnessOptions::new(faux_model(), session)
    };
    Arc::new(AgentHarness::new(options))
}

fn executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

fn daemon_ctx(harness: &Arc<AgentHarness>, executor: Arc<TriggerExecutor>) -> DaemonCtx {
    DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor,
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
        inherit_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

fn command_ctx<'a>(
    extra: &'a DaemonCtx,
    cwd: &'a Path,
) -> CommandCtx<'a, DaemonCtx> {
    CommandCtx {
        session_id: "test-session",
        log_path: None,
        tool_count: 0,
        cwd,
        extra,
    }
}

fn setup_with_skills(
    skills: Vec<Skill>,
) -> (tempfile::TempDir, Arc<AgentHarness>, Arc<TriggerExecutor>) {
    let session = new_session();
    let harness = harness_with_skills(session, skills);
    let executor = executor_for(&harness);
    let tmp = tempfile::tempdir().unwrap();
    (tmp, harness, executor)
}

fn install_args(target: &str, confirm: bool, overwrite: bool) -> Vec<String> {
    let mut argv = Vec::new();
    if confirm {
        argv.push("--confirm".into());
    }
    if overwrite {
        argv.push("--overwrite".into());
    }
    argv.push(target.into());
    argv
}

fn err_of<T>(result: Result<T, String>) -> String {
    match result {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    }
}

#[test]
fn parse_skill_install_args_accepts_flags_and_positional_target() {
    let argv = install_args("https://x/skill.md", true, true);
    let parsed = parse_skill_install_args(&argv).unwrap();
    assert_eq!(parsed.target, "https://x/skill.md");
    assert!(parsed.confirm);
    assert!(parsed.overwrite);

    let argv = ["--yes".into(), "foo".into()];
    let parsed = parse_skill_install_args(&argv).unwrap();
    assert!(parsed.confirm);
    assert!(!parsed.overwrite);
}

#[test]
fn parse_skill_install_args_rejects_unknown_option_and_bad_arity() {
    let err = err_of(parse_skill_install_args(&["--bogus".into()]));
    assert!(err.contains("unknown option for /skills install"), "{err}");

    let err = err_of(parse_skill_install_args(&[]));
    assert!(err.contains("usage: /skills install"), "{err}");

    let err = err_of(parse_skill_install_args(&["a".into(), "b".into()]));
    assert!(err.contains("usage: /skills install"), "{err}");
}

#[test]
fn skill_install_source_classifies_urls_and_paths() {
    let cwd = Path::new("/work/project");
    let url = skill_install_source("https://example.com/skill.md", cwd);
    assert_eq!(url["type"], "url");
    assert_eq!(url["url"], "https://example.com/skill.md");

    let path = skill_install_source("skill.md", cwd);
    assert_eq!(path["type"], "path");
    assert_eq!(path["path"], "/work/project/skill.md");

    let abs = skill_install_source("/tmp/skill.md", cwd);
    assert_eq!(abs["type"], "path");
    assert_eq!(abs["path"], "/tmp/skill.md");
}

#[test]
fn parse_skill_remove_args_accepts_source_and_flags() {
    let argv = ["foo".into()];
    let parsed = parse_skill_remove_args(&argv).unwrap();
    assert_eq!(parsed.name, "foo");
    assert!(parsed.source.is_none());
    assert!(!parsed.confirm);

    let argv = ["--confirm".into(), "foo".into(), "user".into()];
    let parsed = parse_skill_remove_args(&argv).unwrap();
    assert_eq!(parsed.name, "foo");
    assert_eq!(parsed.source, Some(SkillSource::User));
    assert!(parsed.confirm);
}

#[test]
fn parse_skill_remove_args_rejects_unknown_option_bad_source_and_bad_arity() {
    let err = err_of(parse_skill_remove_args(&["--bogus".into()]));
    assert!(err.contains("unknown option for /skills remove"), "{err}");

    let err = err_of(parse_skill_remove_args(&["foo".into(), "cloud".into()]));
    assert!(err.contains("invalid skill source"), "{err}");

    let err = err_of(parse_skill_remove_args(&[]));
    assert!(err.contains("usage: /skills remove"), "{err}");

    let err = err_of(parse_skill_remove_args(&["a".into(), "user".into(), "b".into()]));
    assert!(err.contains("usage: /skills remove"), "{err}");
}

#[test]
fn parse_skill_source_accepts_builtin_user_project_only() {
    assert_eq!(parse_skill_source("builtin").unwrap(), SkillSource::Builtin);
    assert_eq!(parse_skill_source("user").unwrap(), SkillSource::User);
    assert_eq!(parse_skill_source("project").unwrap(), SkillSource::Project);
    let err = parse_skill_source("cloud").unwrap_err();
    assert!(err.contains("expected one of: builtin, user, project"), "{err}");
}

#[test]
fn optional_skill_source_maps_none_to_ok_none() {
    assert_eq!(optional_skill_source(None).unwrap(), None);
    assert_eq!(
        optional_skill_source(Some(&"user".into())).unwrap(),
        Some(SkillSource::User)
    );
    assert!(optional_skill_source(Some(&"cloud".into())).is_err());
}

#[test]
fn resolve_active_skill_prefers_unique_name_and_source() {
    let skills = vec![
        sample_skill("foo", SkillSource::User),
        sample_skill("foo", SkillSource::Project),
        sample_skill("bar", SkillSource::User),
    ];
    let found = resolve_active_skill(&skills, "bar", None).unwrap();
    assert_eq!(found.name, "bar");

    let found = resolve_active_skill(&skills, "foo", Some(SkillSource::Project)).unwrap();
    assert_eq!(found.source, SkillSource::Project);

    let err = resolve_active_skill(&skills, "nope", None).unwrap_err();
    assert!(err.contains("no active skill named 'nope'"), "{err}");

    let err = resolve_active_skill(&skills, "foo", None).unwrap_err();
    assert!(err.contains("multiple active skills named 'foo'"), "{err}");
}

#[test]
fn print_skills_list_handles_empty_and_loaded_skills() {
    print_skills_list(&[]);

    let mut disabled = sample_skill("foo", SkillSource::User);
    disabled.disable_model_invocation = true;
    print_skills_list(&[disabled]);
}

#[test]
fn print_install_skill_result_renders_preview_and_normal_text() {
    let result = AgentToolResult {
        content: vec![],
        details: serde_json::json!({
            "phase": "preview",
            "name": "foo",
            "target_path": "/tmp/foo",
            "size": 12,
            "existing": false,
            "overwrite_required": false,
        }),
        terminate: None,
    };
    let args = InstallSkillArgs {
        target: "https://x/skill.md",
        confirm: false,
        overwrite: false,
    };
    print_install_skill_result(&result, &args);

    let result = AgentToolResult {
        content: vec![UserContentBlock::text("installed ok")],
        details: serde_json::json!({"phase": "installed"}),
        terminate: None,
    };
    print_install_skill_result(&result, &args);
}

#[test]
fn print_remove_skill_result_renders_preview_and_normal_text() {
    let result = AgentToolResult {
        content: vec![],
        details: serde_json::json!({
            "phase": "preview",
            "name": "foo",
            "target_path": "/tmp/foo",
        }),
        terminate: None,
    };
    print_remove_skill_result(&result);

    let result = AgentToolResult {
        content: vec![UserContentBlock::text("removed ok")],
        details: serde_json::json!({}),
        terminate: None,
    };
    print_remove_skill_result(&result);
}

#[test]
fn tool_result_text_joins_text_blocks_and_skips_images() {
    let result = AgentToolResult {
        content: vec![
            UserContentBlock::text("first"),
            UserContentBlock::Image(theway_llm_provider::ImageContent {
                data: "b64".into(),
                mime_type: "image/png".into(),
            }),
            UserContentBlock::text("second"),
        ],
        details: serde_json::json!({}),
        terminate: None,
    };
    assert_eq!(tool_result_text(&result), "first\nsecond");
}

#[tokio::test]
async fn skills_list_empty_is_handled() {
    let (tmp, harness, executor) = setup_with_skills(vec![]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn skills_show_requires_name_and_valid_source() {
    let (tmp, harness, executor) = setup_with_skills(vec![sample_skill("foo", SkillSource::User)]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["show".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skills show")));

    let outcome = SkillsCommand.run(&["show".into(), "foo".into(), "cloud".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("invalid skill source")));
}

#[tokio::test]
async fn skills_show_renders_active_skill() {
    let (tmp, harness, executor) = setup_with_skills(vec![sample_skill("foo", SkillSource::User)]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["show".into(), "foo".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn skills_reload_maps_not_configured_error() {
    let session = new_session();
    let harness = harness_with_reload_fn(session, vec![], None);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor.clone());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["reload".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("reload skills failed:")));
}

#[tokio::test]
async fn skills_reload_prints_summary_on_success() {
    let session = new_session();
    let reload: ReloadSkillsFn = Arc::new(|| {
        Box::pin(async {
            LoadSkillsOutput {
                skills: vec![sample_skill("foo", SkillSource::User)],
                diagnostics: vec![],
            }
        })
    });
    let harness = harness_with_reload_fn(session, vec![], Some(reload));
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor.clone());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["reload".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn skills_install_command_validates_args() {
    let (tmp, harness, executor) = setup_with_skills(vec![]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["install".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skills install")));

    let outcome = SkillsCommand.run(&["install".into(), "--bogus".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown option")));
}

#[tokio::test]
async fn skills_install_command_previews_local_skill() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let (tmp, harness, executor) = setup_with_skills(vec![]);
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    // Arrange: write a valid SKILL.md in a temp dir.
    let skill_dir = tmp.path().join("skill-src");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: foo-skill\ndescription: A foo skill\n---\nbody\n",
    )
    .unwrap();
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand
        .run(&["install".into(), skill_dir.join("SKILL.md").to_string_lossy().to_string()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
}

#[tokio::test]
async fn skills_enable_missing_name_and_invalid_source_are_errors() {
    let (tmp, harness, executor) = setup_with_skills(vec![sample_skill("foo", SkillSource::User)]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["enable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skills enable")));

    let outcome = SkillsCommand.run(&["enable".into(), "foo".into(), "cloud".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("invalid skill source")));
}

#[tokio::test]
async fn skills_enable_already_enabled_is_handled() {
    let (tmp, harness, executor) = setup_with_skills(vec![sample_skill("foo", SkillSource::User)]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["enable".into(), "foo".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn skills_disable_persists_and_reloads() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let session = new_session();
    let reload: ReloadSkillsFn = Arc::new(|| {
        Box::pin(async {
            LoadSkillsOutput {
                skills: vec![sample_skill("foo", SkillSource::User)],
                diagnostics: vec![],
            }
        })
    });
    let harness = harness_with_reload_fn(session, vec![sample_skill("foo", SkillSource::User)], Some(reload));
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor.clone());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["disable".into(), "foo".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
    assert!(base.path().join("skill-overrides.json").exists());
}

#[tokio::test]
async fn skills_disable_reload_error_is_mapped() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let session = new_session();
    let harness = harness_with_reload_fn(session, vec![sample_skill("foo", SkillSource::User)], None);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor.clone());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["disable".into(), "foo".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("reload after skill state change failed:")));
}

#[tokio::test]
async fn skills_remove_command_validates_args() {
    let (tmp, harness, executor) = setup_with_skills(vec![sample_skill("foo", SkillSource::User)]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["remove".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skills remove")));

    let outcome = SkillsCommand.run(&["remove".into(), "--bogus".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown option")));
}

#[tokio::test]
async fn skills_unknown_subcommand_returns_usage_error() {
    let (tmp, harness, executor) = setup_with_skills(vec![]);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillsCommand.run(&["bogus".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skills")));
}
