//! Periodic dynamic hook checks, audit-only matches, and the home fixture flow.

use std::sync::Arc;
use std::time::{Duration, Instant};

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentTool, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{Message, UserContent};

use super::helpers::*;
use super::triggers;

#[tokio::test(flavor = "current_thread")]
async fn audit_only_match_is_not_promoted_when_other_rule_requests_chat_promotion() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let audit_rule = triggers::global_registry()
        .add_rule_with_flags(
            "the event says build finished",
            "echo dynamic-fired",
            true,
            false,
        )
        .expect("audit rule");
    let promote_rule = triggers::global_registry()
        .add_rule_with_flags(
            "the event says deploy finished",
            "echo deploy-fired",
            true,
            true,
        )
        .expect("promote rule");

    let bash_calls: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![Arc::new(RecordingBashTool::new(bash_calls.clone())) as Arc<dyn AgentTool>];
    opts.stream_fn = Some(dynamic_trigger_stream());
    let before_trigger_action = Some(triggers::before_trigger_action_hook(
        triggers::global_registry().clone(),
    ));
    let stream_fn = opts.stream_fn.clone();
    let harness = AgentHarness::new(opts);
    let executor = std::sync::Arc::new(theway::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        theway::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events: Arc<parking_lot::Mutex<Vec<theway::trigger_engine::event::TriggerEvent>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let event_sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |event| {
        event_sink.lock().push(event);
    }));

    let _ = executor.handle_trigger(sample_event_trigger()).await;
    assert!(
        wait_for_completed(&events, "trace-dynamic-e2e").await,
        "dynamic trigger sub-agent should complete"
    );
    assert_eq!(bash_calls.lock().as_slice(), ["echo dynamic-fired"]);

    let parent_messages = harness.agent().state().messages.clone();
    assert!(
        !parent_messages.iter().any(|message| {
            matches!(
                message,
                theway_core::AgentMessage::Llm(Message::User(user))
                    if matches!(&user.content, UserContent::Text(text) if text.contains("[Trigger trace-dynamic-e2e]"))
            )
        }),
        "audit-only matched rule {} must not be promoted just because {} requested promotion: {parent_messages:#?}",
        audit_rule.id,
        promote_rule.id
    );
}

#[tokio::test(flavor = "current_thread")]
async fn home_helloworld_trigger_prints_file_contents() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _dynamic_guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let home = tempfile::tempdir().expect("home tempdir");
    std::fs::write(home.path().join("helloworld"), "hello from home e2e").expect("write fixture");
    let _home_guard = EnvGuard::set("HOME", home.path().to_str().expect("home path"));

    let bash_calls: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![
        Arc::new(triggers::NewTriggerTool) as Arc<dyn AgentTool>,
        Arc::new(HomeFileBashTool::new(bash_calls.clone())) as Arc<dyn AgentTool>,
    ];
    opts.stream_fn = Some(dynamic_trigger_stream());
    let before_trigger_action = Some(triggers::before_trigger_action_hook(
        triggers::global_registry().clone(),
    ));
    let stream_fn = opts.stream_fn.clone();
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = std::sync::Arc::new(theway::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        theway::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));
    let _fire_once_unsub = executor.subscribe(triggers::fire_once_trigger_listener(
        triggers::global_registry().clone(),
    ));
    executor.register_notification_hook(Arc::new(
        triggers::DynamicTriggerCheckHook::with_interval(
            triggers::global_registry().clone(),
            Duration::from_millis(10),
        ),
    ));

    let user_request = concat!(
        "\u{5f53} $home \u{76ee}\u{5f55}\u{4e0b}\u{6709}\u{4e2a} helloworld ",
        "\u{6587}\u{4ef6}\u{ff0c}\u{90a3}\u{4e48}\u{5c31}\u{6253}\u{5370}",
        "\u{5b83}\u{7684}\u{5185}\u{5bb9}\u{51fa}\u{6765}"
    );
    harness
        .prompt(user_request)
        .await
        .expect("prompt should create home trigger");

    let rules = triggers::global_registry().list();
    assert_eq!(rules.len(), 1);
    assert!(rules[0].condition.contains("helloworld"));
    assert!(rules[0].action.contains("$HOME/helloworld"));

    assert!(
        wait_for_bash_call(
            &bash_calls,
            "test -f \"$HOME/helloworld\" && cat \"$HOME/helloworld\""
        )
        .await,
        "periodic dynamic check should inspect and print the home fixture"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let rules = triggers::global_registry().list();
        if !rules[0].enabled {
            assert!(
                rules[0].fired_at.is_some(),
                "fire_once rule should record fired_at"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fire_once rule was not disabled after successful trigger"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entries = session.entries().await.expect("session entries");
        let summaries = any_trigger_result_summary(&entries);
        if let Some(summary) = summaries
            .iter()
            .find(|summary| summary.contains("hello from home e2e"))
        {
            assert!(summary.contains("hello from home e2e"), "{summary}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "trigger_result summary did not include file contents: {summaries:#?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn periodic_dynamic_hook_checks_rules_and_executes_matching_action() {
    let registry = triggers::dynamic::DynamicTriggerRegistry::new();
    registry
        .add_rule("a dynamic periodic check arrives", "echo periodic-fired")
        .expect("rule");

    let bash_calls: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![Arc::new(RecordingBashTool::new(bash_calls.clone())) as Arc<dyn AgentTool>];
    opts.stream_fn = Some(dynamic_trigger_stream());
    let before_trigger_action = Some(triggers::before_trigger_action_hook(registry.clone()));
    let stream_fn = opts.stream_fn.clone();
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = std::sync::Arc::new(theway::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        theway::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));
    executor.register_notification_hook(Arc::new(
        triggers::DynamicTriggerCheckHook::with_interval(registry, Duration::from_millis(10)),
    ));

    assert!(
        wait_for_bash_call(&bash_calls, "echo periodic-fired").await,
        "periodic dynamic hook should emit a check trigger that executes the matching rule"
    );
}
