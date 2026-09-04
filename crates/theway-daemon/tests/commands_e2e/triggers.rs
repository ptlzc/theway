//! `/triggers` + `/cron` command suites: status, remove, enable/disable, abort,
//! and the cron registry add/list/toggle/remove flows with secret redaction.
//! (The `/new-trigger` control-plane approval path lives in `control_plane.rs`.)

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentTool, MemorySessionStorage, Session, SessionStorage,
    SessionTreeEntry,
};

use super::helpers::*;
use crate::commands;
use crate::triggers;

#[tokio::test]
async fn dispatch_triggers_status_is_read_only_and_available() {
    // Serialize with the other trigger tests: they share the process-global rule registry, so
    // an unlocked `clear_for_tests()` here can wipe another test's rule mid-run.
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.tools = vec![Arc::new(triggers::NewTriggerTool::new(
        triggers::global_registry().clone(),
    )) as Arc<dyn AgentTool>];
    opts.stream_fn = Some(new_trigger_extraction_stream());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/triggers", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(
        session.entries().await.unwrap().is_empty(),
        "/triggers status must not mutate the session"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_triggers_remove_deletes_dynamic_rule() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();
    let rule = triggers::global_registry()
        .add_rule("event says delete this", "echo deleted")
        .expect("rule");

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome =
        commands::dispatch(&format!("/triggers remove {}", rule.id), &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(triggers::global_registry().list().is_empty());
    assert!(
        session.entries().await.unwrap().is_empty(),
        "/triggers remove only mutates the dynamic rule registry"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_triggers_disable_and_enable_updates_rule_state() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();
    let rule = triggers::global_registry()
        .add_rule("event says toggle this", "echo toggled")
        .expect("rule");

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome =
        commands::dispatch(&format!("/triggers disable {}", rule.id), &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(!triggers::global_registry().list()[0].enabled);

    let outcome =
        commands::dispatch(&format!("/triggers enable {}", rule.id), &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(triggers::global_registry().list()[0].enabled);
    assert!(
        session.entries().await.unwrap().is_empty(),
        "/triggers enable/disable only mutates the dynamic rule registry"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_cron_add_lists_toggles_and_removes_job() {
    let _guard = CRON_LOCK.lock().unwrap();
    triggers::global_cron_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch(
        "/cron add \"*/10 * * * *\" summarize the repo state",
        &registry,
        &ctx,
    )
    .await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    let jobs = triggers::global_cron_registry().list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].schedule, "*/10 * * * *");
    assert_eq!(jobs[0].action, "summarize the repo state");
    assert!(jobs[0].enabled);

    let list = commands::dispatch("/cron list", &registry, &ctx).await;
    assert!(matches!(list, commands::CommandOutcome::Handled));
    let rendered =
        commands::render_cron_jobs(&[triggers::global_cron_registry().list()[0].clone()])
            .join("\n");
    assert!(
        rendered.contains("Cron jobs (session, 1):"),
        "cron list should label session scope: {rendered}"
    );
    assert!(rendered.contains("summarize the repo state"));

    let disable =
        commands::dispatch(&format!("/cron disable {}", jobs[0].id), &registry, &ctx).await;
    assert!(matches!(disable, commands::CommandOutcome::Handled));
    assert!(!triggers::global_cron_registry().list()[0].enabled);

    let enable = commands::dispatch(&format!("/cron enable {}", jobs[0].id), &registry, &ctx).await;
    assert!(matches!(enable, commands::CommandOutcome::Handled));
    assert!(triggers::global_cron_registry().list()[0].enabled);

    let remove = commands::dispatch(&format!("/cron remove {}", jobs[0].id), &registry, &ctx).await;
    assert!(matches!(remove, commands::CommandOutcome::Handled));
    assert!(triggers::global_cron_registry().list().is_empty());
    let entries = session.entries().await.unwrap();
    let audits = entries
        .iter()
        .filter_map(|entry| match entry {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "cron_control_plane" => data.as_ref(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        audits.len(),
        4,
        "cron writes should be audited: {entries:#?}"
    );
    assert_eq!(audits[0].get("op").and_then(|v| v.as_str()), Some("add"));
    assert_eq!(
        audits[0].get("actor").and_then(|v| v.as_str()),
        Some("slash")
    );
    assert_eq!(
        audits[0].get("after_enabled").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(
        audits[0].get("next_run").and_then(|v| v.as_str()).is_some(),
        "enabled cron audit should include next_run: {:#?}",
        audits[0]
    );
    assert_eq!(
        audits[1].get("op").and_then(|v| v.as_str()),
        Some("disable")
    );
    assert_eq!(
        audits[1].get("before_enabled").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        audits[1].get("after_enabled").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(audits[2].get("op").and_then(|v| v.as_str()), Some("enable"));
    assert_eq!(audits[3].get("op").and_then(|v| v.as_str()), Some("remove"));
    assert_eq!(
        audits[3].get("removed").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_cron_list_redacts_secret_like_action_preview() {
    let _guard = CRON_LOCK.lock().unwrap();
    triggers::global_cron_registry().clear_for_tests();
    let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
    triggers::global_cron_registry()
        .add_job("* * * * *", &format!("use {secret}"))
        .unwrap();

    let rendered = commands::render_cron_jobs(&triggers::global_cron_registry().list()).join("\n");
    assert!(!rendered.contains(secret), "{rendered}");
    assert!(rendered.contains("[REDACTED:"), "{rendered}");
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_cron_add_audit_redacts_secret_like_action_preview() {
    let _guard = CRON_LOCK.lock().unwrap();
    triggers::global_cron_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
    let outcome = commands::dispatch(
        &format!("/cron add \"0 * * * *\" call API with Bearer abcdefghijklmnop and {secret}"),
        &registry,
        &ctx,
    )
    .await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let entries = session.entries().await.unwrap();
    let audit = entries
        .iter()
        .find_map(|entry| match entry {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "cron_control_plane" => data.as_ref(),
            _ => None,
        })
        .expect("cron add should write audit");
    let serialized = serde_json::to_string(audit).unwrap();
    assert!(!serialized.contains(secret), "{serialized}");
    assert!(
        !serialized.contains("Bearer abcdefghijklmnop"),
        "{serialized}"
    );
    assert!(serialized.contains("[REDACTED:"), "{serialized}");
}

#[tokio::test]
async fn dispatch_triggers_abort_missing_trace_returns_error() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/triggers abort missing-trace", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(message) => {
            assert!(message.contains("no running trigger"));
            assert!(message.contains("missing-trace"));
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
    assert!(
        session.entries().await.unwrap().is_empty(),
        "failed abort lookup must not mutate the session"
    );
}

#[tokio::test]
async fn dispatch_triggers_abort_all_empty_harness_is_handled_and_read_only() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/triggers abort --all", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(
        session.entries().await.unwrap().is_empty(),
        "abort --all on an empty harness must not mutate the session"
    );
}
