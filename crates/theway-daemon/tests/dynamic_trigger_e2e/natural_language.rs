//! Dynamic trigger / cron job creation from natural-language prompts.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentTool, MemorySessionStorage, Session, SessionStorage,
};

use super::helpers::*;
use super::triggers;

#[tokio::test(flavor = "current_thread")]
async fn natural_language_prompt_creates_dynamic_trigger_and_runtime_event_executes_action() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();
    triggers::global_cron_registry().clear_for_tests();

    let bash_calls: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![
        Arc::new(triggers::NewTriggerTool::new(
            triggers::global_registry().clone(),
        )) as Arc<dyn AgentTool>,
        Arc::new(RecordingBashTool::new(bash_calls.clone())) as Arc<dyn AgentTool>,
    ];
    opts.stream_fn = Some(dynamic_trigger_stream());
    let before_trigger_action = Some(triggers::before_trigger_action_hook(
        triggers::global_registry().clone(),
    ));
    let stream_fn = opts.stream_fn.clone();
    let harness = AgentHarness::new(opts);
    let executor = std::sync::Arc::new(
        theway_daemon::trigger_engine::execution::TriggerExecutor::new(
            harness.agent_arc(),
            harness.session().clone(),
            theway_daemon::trigger_engine::runtime::TriggerRuntimeConfig::default(),
            None,
            None,
            before_trigger_action,
            stream_fn,
            None,
            None,
        ),
    );

    harness
        .prompt("Create a trigger: when the event says build finished, run echo dynamic-fired")
        .await
        .expect("prompt should create dynamic trigger");

    let rules = triggers::global_registry().list();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].condition, "the event says build finished");
    assert_eq!(rules[0].action, "echo dynamic-fired");
    assert!(
        triggers::global_cron_registry().list().is_empty(),
        "event/condition trigger request must not create a cron job"
    );

    let events: Arc<parking_lot::Mutex<Vec<theway_daemon::trigger_engine::event::TriggerEvent>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let event_sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |event| {
        event_sink.lock().push(event);
    }));
    let _fire_once_unsub = executor.subscribe(triggers::fire_once_trigger_listener(
        triggers::global_registry().clone(),
    ));

    let _ = executor.handle_trigger(sample_event_trigger()).await;
    assert!(
        wait_for_completed(&events, "trace-dynamic-e2e").await,
        "dynamic trigger sub-agent should complete"
    );
    assert_eq!(bash_calls.lock().as_slice(), ["echo dynamic-fired"]);
    let rules = triggers::global_registry().list();
    assert_eq!(rules.len(), 1);
    assert!(!rules[0].enabled, "fire_once rule should be disabled");
    assert!(
        rules[0].fired_at.is_some(),
        "fire_once rule should record fired_at"
    );

    let entries = session.entries().await.expect("session entries");
    assert!(
        entries.iter().any(|entry| {
            matches!(
                entry,
                theway_core::SessionTreeEntry::Custom { custom_type, data, .. }
                    if custom_type == "trigger_result"
                        && data
                            .as_ref()
                            .and_then(|d| d.get("trace_id"))
                            .and_then(|v| v.as_str())
                            == Some("trace-dynamic-e2e")
            )
        }),
        "trigger_result audit should be written: {entries:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn natural_language_scheduled_job_creates_cron_not_dynamic_trigger_chinese() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();
    triggers::global_cron_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![
        Arc::new(triggers::NewCronJobTool::new(
            None,
            triggers::global_cron_registry().clone(),
        )) as Arc<dyn AgentTool>,
        Arc::new(triggers::NewTriggerTool::new(
            triggers::global_registry().clone(),
        )) as Arc<dyn AgentTool>,
    ];
    opts.stream_fn = Some(dynamic_trigger_stream());
    let before_trigger_action: Option<
        theway_daemon::trigger_engine::execution::BeforeTriggerActionHook,
    > = None;
    let stream_fn = opts.stream_fn.clone();
    let harness = AgentHarness::new(opts);
    let _executor = std::sync::Arc::new(
        theway_daemon::trigger_engine::execution::TriggerExecutor::new(
            harness.agent_arc(),
            harness.session().clone(),
            theway_daemon::trigger_engine::runtime::TriggerRuntimeConfig::default(),
            None,
            None,
            before_trigger_action,
            stream_fn,
            None,
            None,
        ),
    );

    harness
        .prompt("创建一个每小时的定时任务，查看下 hackernews 首页新闻")
        .await
        .expect("prompt should create cron job");

    let jobs = triggers::global_cron_registry().list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].schedule, "0 * * * *");
    assert!(jobs[0].action.contains("Hacker News"));
    assert!(
        triggers::global_registry().list().is_empty(),
        "scheduled job must not create a dynamic trigger"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn natural_language_scheduled_job_creates_cron_not_dynamic_trigger_english() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();
    triggers::global_cron_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![
        Arc::new(triggers::NewCronJobTool::new(
            None,
            triggers::global_cron_registry().clone(),
        )) as Arc<dyn AgentTool>,
        Arc::new(triggers::NewTriggerTool::new(
            triggers::global_registry().clone(),
        )) as Arc<dyn AgentTool>,
    ];
    opts.stream_fn = Some(dynamic_trigger_stream());
    let before_trigger_action: Option<
        theway_daemon::trigger_engine::execution::BeforeTriggerActionHook,
    > = None;
    let stream_fn = opts.stream_fn.clone();
    let harness = AgentHarness::new(opts);
    let _executor = std::sync::Arc::new(
        theway_daemon::trigger_engine::execution::TriggerExecutor::new(
            harness.agent_arc(),
            harness.session().clone(),
            theway_daemon::trigger_engine::runtime::TriggerRuntimeConfig::default(),
            None,
            None,
            before_trigger_action,
            stream_fn,
            None,
            None,
        ),
    );

    harness
        .prompt("Create an hourly scheduled job to check Hacker News")
        .await
        .expect("prompt should create cron job");

    let jobs = triggers::global_cron_registry().list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].schedule, "0 * * * *");
    assert!(
        triggers::global_registry().list().is_empty(),
        "scheduled job must not create a dynamic trigger"
    );
}
