//! Delivery action hooks — before_trigger_action outcomes: no-promote stability,
//! inject_summary, and inject_and_run dispatch (idle vs streaming parent).

use super::*;

pub(crate) fn promoting_action_hook(
    template: Option<String>,
    require_approval: bool,
) -> theway::trigger_engine::execution::BeforeTriggerActionHook {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
    };
    let hook: BeforeTriggerActionHook =
        Arc::new(move |ctx: BeforeTriggerActionContext, _cancel| {
            let template = template.clone();
            Box::pin(async move {
                TriggerAction {
                    prompt: format!(
                        "{} fired: {}",
                        ctx.trigger.source_label, ctx.trigger.event_label
                    ),
                    promote: PromoteAction::PromoteSummaryNow {
                        template_body: template,
                    },
                    promote_requires_approval: require_approval,
                    delivery: theway::trigger_engine::execution::TriggerDelivery::SubAgent,
                }
            })
        });
    hook
}

/// Acceptance #8 from the #20 amendment: no `PromoteAction` configured → parent transcript
/// stays stable. Only `trigger` + `trigger_result` Custom entries appear; no message-typed
/// entries beyond the user prompt + sub-agent transcript (which goes to sub-session, not
/// parent).
#[tokio::test]
async fn no_promote_action_leaves_parent_transcript_stable() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-no-promote", "trace-no-promote"))
        .await;
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-no-promote" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("must complete");

    let entries = session.entries().await.unwrap();
    // Must NOT contain any Message entries (sub-agent transcript lives in sub-session,
    // promotion didn't fire so nothing inserted).
    let has_message = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(
        !has_message,
        "no promote → parent transcript MUST be empty of Message entries; got {} entries",
        entries.len()
    );
    // Must NOT have a trigger_promotion audit either.
    let has_promotion_audit = entries.iter().any(|e| {
        matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        )
    });
    assert!(
        !has_promotion_audit,
        "no promote → no trigger_promotion audit"
    );

    let evs = events.lock().unwrap().clone();
    let promoted = evs
        .iter()
        .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. }));
    let pending = evs
        .iter()
        .any(|e| matches!(e, TriggerEvent::PromotionPending { .. }));
    assert!(!promoted && !pending, "no promote → no promotion events");
}

/// Inject-summary delivery hook: returns [`TriggerDelivery::InjectSummary`] with a verbatim
/// `{{trigger.payload_summary}}` template (the source-as-feed shape used by MCP servers
/// configured with `inject_summary = true`). Falls back to `PromoteAction::None` when the
/// push carried no summary.
fn inject_summary_action_hook() -> theway::trigger_engine::execution::BeforeTriggerActionHook {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
        TriggerDelivery,
    };
    let hook: BeforeTriggerActionHook =
        Arc::new(move |ctx: BeforeTriggerActionContext, _cancel| {
            let has_summary = ctx.trigger.payload_summary.is_some();
            Box::pin(async move {
                TriggerAction {
                    prompt: String::new(),
                    promote: if has_summary {
                        PromoteAction::PromoteSummaryNow {
                            template_body: Some("{{trigger.payload_summary}}".into()),
                        }
                    } else {
                        PromoteAction::None
                    },
                    promote_requires_approval: false,
                    delivery: TriggerDelivery::InjectSummary,
                }
            })
        });
    hook
}

/// Inject-summary delivery skips the sub-agent entirely and injects `payload_summary`
/// verbatim (with the engine-enforced `[Trigger ]` prefix). The `trigger_result` audit
/// records `message_count: 0` + `delivery: "inject_summary"` + `cost_usd: 0.0` — proof no
/// sub-agent ran. Even with a `stream_fn` configured, its output must never appear.
#[tokio::test]
async fn inject_summary_skips_subagent_and_injects_payload_summary() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("SUBAGENT RAN — must not appear"));
    before_trigger_action = Some(inject_summary_action_hook());
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-inject", "trace-inject"))
        .await;

    let inserted_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-inject" => Some(inserted_entry_id.clone()),
            _ => None,
        })
    })
    .await
    .expect("inject must promote");

    let entries = session.entries().await.unwrap();

    // Injected Message::User body = payload summary verbatim, with the engine prefix.
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_id => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inject must insert a parent user message");
    assert!(
        body.starts_with("[Trigger "),
        "must carry engine [Trigger] prefix: {body}"
    );
    assert!(
        body.contains("PR #42 merged"),
        "must inject verbatim payload summary: {body}"
    );
    assert!(
        !body.contains("SUBAGENT RAN"),
        "sub-agent output must not appear in an inject delivery: {body}"
    );

    // trigger_result audit proves the sub-agent path was skipped.
    let result_audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_result" => Some(d.clone()),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    assert_eq!(result_audit["message_count"], serde_json::json!(0));
    assert_eq!(
        result_audit["delivery"],
        serde_json::json!("inject_summary")
    );
    assert_eq!(result_audit["cost_usd"], serde_json::json!(0.0));
}

/// Inject-summary with a `None` payload summary promotes nothing (no `Message` inserted, no
/// `trigger_promotion` audit) but still completes via the inject path (no sub-agent).
#[tokio::test]
async fn inject_summary_without_payload_summary_promotes_nothing() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("unused"));
    before_trigger_action = Some(inject_summary_action_hook());
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let mut trigger = sample_trigger("k-inject-none", "trace-inject-none");
    trigger.payload_summary = None;
    let _ = executor.handle_trigger(trigger).await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-inject-none" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("inject path must still complete");

    let entries = session.entries().await.unwrap();
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "no summary → nothing injected"
    );
    assert!(
        !entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        )),
        "no summary → no trigger_promotion audit"
    );
    let evs = events.lock().unwrap().clone();
    assert!(
        !evs.iter()
            .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. })),
        "no summary → no TriggerPromoted"
    );
}

/// Inject-and-run delivery hook: injects `prompt` into the parent conversation and runs one
/// turn in the parent's context (`TriggerDelivery::InjectAndRun`).
fn inject_and_run_action_hook(
    prompt: &'static str,
) -> theway::trigger_engine::execution::BeforeTriggerActionHook {
    use theway::trigger_engine::execution::{
        BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
        TriggerDelivery,
    };
    let hook: BeforeTriggerActionHook =
        Arc::new(move |_ctx: BeforeTriggerActionContext, _cancel| {
            Box::pin(async move {
                TriggerAction {
                    prompt: prompt.to_string(),
                    promote: PromoteAction::None,
                    promote_requires_approval: false,
                    delivery: TriggerDelivery::InjectAndRun,
                }
            })
        });
    hook
}

/// Inject-and-run against an IDLE parent: the prompt is appended to the parent conversation
/// (with the `[Trigger ]` prefix) and `TriggerRequestsMainRun` is emitted — but the kernel
/// must NOT run the model itself (the embedder owns the single-tenant parent agent).
#[tokio::test]
async fn inject_and_run_idle_appends_prompt_and_requests_main_run() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("MODEL RAN — must not appear"));
    before_trigger_action = Some(inject_and_run_action_hook("check if I need an umbrella"));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-iar", "trace-iar"))
        .await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerRequestsMainRun { trace_id } if trace_id == "trace-iar" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("idle inject_and_run must emit TriggerRequestsMainRun");

    let entries = session.entries().await.unwrap();

    // Injected user message = prompt verbatim with the engine `[Trigger ]` prefix.
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } => match &u.content {
                theway_llm_provider::UserContent::Text(s)
                    if s.contains("check if I need an umbrella") =>
                {
                    Some(s.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("inject_and_run must append a parent user message");
    assert!(
        body.starts_with("[Trigger "),
        "must carry engine [Trigger] prefix: {body}"
    );

    // The kernel must NOT run the model: no assistant message, parent stays idle.
    assert!(
        !entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
                ..
            }
        )),
        "kernel must not run the model on the idle parent — that is the embedder's job"
    );
    assert!(
        !harness.agent().is_streaming(),
        "parent must remain idle; the kernel only requested a run"
    );

    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_result" => Some(d.clone()),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    assert_eq!(audit["delivery"], serde_json::json!("inject_and_run"));
    assert_eq!(audit["message_count"], serde_json::json!(0));
    assert_eq!(audit["run_dispatch"], serde_json::json!("main_run_request"));
}

/// Inject-and-run against a STREAMING parent: the prompt is enqueued as a follow-up (the
/// in-flight loop will run it) and `TriggerRequestsMainRun` is NOT emitted — there is already
/// a loop to pick the message up, so the embedder need not schedule anything.
#[tokio::test]
async fn inject_and_run_while_streaming_enqueues_follow_up_no_main_run_event() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    let release = Arc::new(Notify::new());
    let stream_fn_val: StreamFn = {
        let release = release.clone();
        let counter = Arc::new(AtomicUsize::new(0));
        Arc::new(move |_, _, _| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let release = release.clone();
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                if n == 0 {
                    release.notified().await;
                }
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text("resp")],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                };
                sender.push(AssistantMessageEvent::Start {
                    partial: msg.clone(),
                });
                sender.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message: msg,
                });
            });
            stream
        })
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(stream_fn_val);
    before_trigger_action = Some(inject_and_run_action_hook("react to the event"));
    let mut harness_opts = AgentHarnessOptions::new(faux_model(), session.clone());
    harness_opts.stream_fn = stream_fn.clone();
    let harness = Arc::new(AgentHarness::new(harness_opts));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        None,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let harness_clone = harness.clone();
    let parent_task = tokio::spawn(async move { harness_clone.prompt("kick off parent").await });
    for _ in 0..200 {
        if harness.agent().is_streaming() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        harness.agent().is_streaming(),
        "parent must be streaming before we fire the trigger"
    );

    let _ = executor
        .handle_trigger(sample_trigger("k-iar-s", "trace-iar-s"))
        .await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-iar-s" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("inject_and_run must complete");

    // Streaming → routed through the follow-up queue, NOT a main-run request.
    let evs = events.lock().unwrap().clone();
    assert!(
        !evs.iter().any(|e| matches!(
            e,
            TriggerEvent::TriggerRequestsMainRun { trace_id } if trace_id == "trace-iar-s"
        )),
        "streaming parent already has a loop — must not emit TriggerRequestsMainRun"
    );

    let audit = session
        .entries()
        .await
        .unwrap()
        .into_iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_result" => Some(d),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    assert_eq!(audit["delivery"], serde_json::json!("inject_and_run"));
    assert_eq!(audit["run_dispatch"], serde_json::json!("follow_up"));

    // Release the parent stream so the spawned task completes cleanly.
    release.notify_one();
    let _ = parent_task.await;
}
