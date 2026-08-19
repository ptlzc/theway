//! Tests for `commands::skill_cmd` — split out of src (see docs/rust-test-files.md).

use std::path::Path;
use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, Skill,
    SkillSource,
};
use theway_llm_provider::{Api, Model, Provider};
use theway_transport::commands::{CommandCtx, CommandOutcome};

use super::*;
use crate::commands::DaemonCtx;
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

fn skill(name: &str, disabled: bool, source: SkillSource) -> Skill {
    Skill {
        name: name.into(),
        description: format!("{name} description"),
        file_path: format!("/tmp/{name}/SKILL.md"),
        content: "body".into(),
        disable_model_invocation: disabled,
        source,
    }
}

fn harness_with(session: Session, skills: Vec<Skill>) -> Arc<AgentHarness> {
    let options = AgentHarnessOptions {
        skills,
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

fn setup(skills: Vec<Skill>) -> (tempfile::TempDir, Arc<AgentHarness>, Arc<TriggerExecutor>) {
    let tmp = tempfile::tempdir().unwrap();
    let harness = harness_with(new_session(), skills);
    let executor = executor_for(&harness);
    (tmp, harness, executor)
}

#[test]
fn skill_command_metadata_is_stable() {
    assert_eq!(SkillCommand.name(), "skill");
    assert!(SkillCommand.description().contains("attach a loaded skill"));
    assert_eq!(SkillCommand.usage(), "<name>");
}

#[tokio::test]
async fn skill_command_requires_exactly_one_name() {
    let (tmp, harness, executor) = setup(vec![]);
    let extra = DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor.clone(),
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
    };
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skill <name>")));

    let outcome = SkillCommand.run(&["a".into(), "b".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /skill <name>")));
}

#[tokio::test]
async fn skill_command_attaches_loaded_skill() {
    let (tmp, harness, executor) = setup(vec![skill("review-pr", false, SkillSource::User)]);
    let extra = DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor.clone(),
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
    };
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillCommand.run(&["review-pr".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::AttachSkill { name } if name == "review-pr"));
}

#[tokio::test]
async fn skill_command_rejects_disabled_skill() {
    let (tmp, harness, executor) = setup(vec![skill("review-pr", true, SkillSource::User)]);
    let extra = DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor.clone(),
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
    };
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillCommand.run(&["review-pr".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("disabled (disable_model_invocation=true)")));
}

#[tokio::test]
async fn skill_command_suggests_prefix_and_contains_matches() {
    let (tmp, harness, executor) = setup(vec![
        skill("review-pr", false, SkillSource::User),
        skill("lint-rust", false, SkillSource::User),
        skill("daily-rust-digest", false, SkillSource::User),
    ]);
    let extra = DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor.clone(),
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
    };
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillCommand.run(&["review".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("Did you mean: review-pr?")));

    let outcome = SkillCommand.run(&["rust".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("Did you mean: lint-rust, daily-rust-digest?")));
}

#[tokio::test]
async fn skill_command_unknown_name_has_no_hint() {
    let (tmp, harness, executor) = setup(vec![skill("review-pr", false, SkillSource::User)]);
    let extra = DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor.clone(),
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
    };
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SkillCommand.run(&["zzz".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no skill named 'zzz'") && !msg.contains("Did you mean")));
}
