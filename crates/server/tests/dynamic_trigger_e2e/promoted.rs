//! Promoted dynamic trigger results entering the parent chat context, and the
//! trigger sub-agent inheriting the parent skill catalog.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentTool, MemorySessionStorage, Session, SessionStorage,
    Skill, SkillSource,
};
use theway_llm_provider::{Message, UserContent};

use super::helpers::*;
use super::triggers;

#[tokio::test(flavor = "current_thread")]
async fn promoted_dynamic_trigger_result_enters_parent_chat_context() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let bash_calls: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.tools = vec![
        Arc::new(triggers::NewTriggerTool) as Arc<dyn AgentTool>,
        Arc::new(RecordingBashTool::new(bash_calls.clone())) as Arc<dyn AgentTool>,
    ];
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

    harness
        .prompt(
            "Create a trigger: when the event says build finished, run echo dynamic-fired, and make the result visible to future turns",
        )
        .await
        .expect("prompt should create promoted dynamic trigger");

    let rules = triggers::global_registry().list();
    assert_eq!(rules.len(), 1);
    assert!(rules[0].promote_to_chat);

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

    let parent_messages = harness.agent().state().messages.clone();
    assert!(
        parent_messages.iter().any(|message| {
            matches!(
                message,
                theway_core::AgentMessage::Llm(Message::User(user))
                    if matches!(&user.content, UserContent::Text(text) if text.contains("[Trigger trace-dynamic-e2e]") && text.contains("matched dyn-"))
            )
        }),
        "promoted trigger result should be present in parent agent context: {parent_messages:#?}"
    );

    let entries = session.entries().await.expect("session entries");
    assert!(
        entries.iter().any(|entry| {
            matches!(
                entry,
                theway_core::SessionTreeEntry::Custom { custom_type, data, .. }
                    if custom_type == "trigger_promotion"
                        && data
                            .as_ref()
                            .and_then(|d| d.get("state"))
                            .and_then(|v| v.as_str())
                            == Some("success")
            )
        }),
        "promotion audit should be written: {entries:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn trigger_sub_agent_sees_parent_skill_catalog() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();
    triggers::global_registry()
        .add_rule(
            "the event says build finished",
            "echo dynamic-fired after considering available skills",
        )
        .expect("rule");

    let seen_system_prompts: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let bash_calls: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.skills = vec![Skill {
        name: "alpha".into(),
        description: "handles alpha workflows".into(),
        file_path: "/tmp/skills/alpha/SKILL.md".into(),
        content: "Alpha skill body.".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }];
    opts.tools = vec![Arc::new(RecordingBashTool::new(bash_calls.clone())) as Arc<dyn AgentTool>];
    opts.stream_fn = Some(recording_dynamic_trigger_stream(
        seen_system_prompts.clone(),
    ));
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

    let prompts = seen_system_prompts.lock().clone();
    assert!(
        prompts.iter().any(|prompt| {
            prompt.contains("<skills>")
                && prompt.contains("- name: alpha")
                && prompt.contains("description: handles alpha workflows")
        }),
        "trigger sub-agent should inherit the parent skill catalog in its system prompt: {prompts:#?}"
    );
}
