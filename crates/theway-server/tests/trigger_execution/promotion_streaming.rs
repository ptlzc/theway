//! Promotion while the parent is streaming — follow-up queue single-write ordering.

use super::*;

/// Promotion fired while the parent agent is mid-stream MUST NOT double-persist or land
/// out of order. Pins QA's PR #67 blocker: the streaming branch hands off to the loop's
/// follow-up queue (single persistence path via the session listener); audit reflects
/// `state: "queued"` and `inserted_entry_id: null` because the entry ID is only known
/// after the loop drains. Once the parent stream releases, the session must contain
/// exactly one promoted Message::User AND it must come AFTER the parent's assistant
/// response — never before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promote_while_parent_is_streaming_routes_through_follow_up_single_write() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    // Controllable stream factory. The first call (parent's initial prompt) waits on
    // `release` so we can race a trigger promotion against the in-flight stream. All
    // later calls (sub-agent inside `handle_trigger`, parent's follow-up turn) resolve
    // immediately so the test doesn't deadlock waiting on them.
    fn controllable_stream_fn(release: Arc<Notify>) -> StreamFn {
        let counter = Arc::new(AtomicUsize::new(0));
        Arc::new(move |_, _, _| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let release = release.clone();
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                if n == 0 {
                    release.notified().await;
                }
                let body = match n {
                    0 => "parent response",
                    _ => "auxiliary response",
                };
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text(body)],
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
    }

    let release = Arc::new(Notify::new());
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(controllable_stream_fn(release.clone()));
    let parent_stream_fn = stream_fn.clone();
    before_trigger_action = Some({
        let hook: theway::trigger_engine::execution::BeforeTriggerActionHook = Arc::new(
            move |ctx: theway::trigger_engine::execution::BeforeTriggerActionContext, _cancel| {
                Box::pin(async move {
                    theway::trigger_engine::execution::TriggerAction {
                        prompt: format!(
                            "{} fired: {}",
                            ctx.trigger.source_label, ctx.trigger.event_label
                        ),
                        // `PromoteSummaryNow` always fires (no conditional gate); we're
                        // testing the persistence/ordering branch in `apply_promotion`,
                        // not the condition evaluator.
                        promote:
                            theway::trigger_engine::execution::PromoteAction::PromoteSummaryNow {
                                template_body: None,
                            },
                        promote_requires_approval: false,
                        delivery: theway::trigger_engine::execution::TriggerDelivery::SubAgent,
                    }
                })
            },
        );
        hook
    });
    let mut harness_opts = AgentHarnessOptions::new(faux_model(), session.clone());
    harness_opts.stream_fn = parent_stream_fn;
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

    // Spawn parent prompt in background; it'll block at the stream's first `notified().await`
    // until we release. `is_streaming()` should be true during this window.
    let harness_clone = harness.clone();
    let parent_task = tokio::spawn(async move { harness_clone.prompt("kick off parent").await });

    // Wait for the parent to actually enter the streaming state.
    for _ in 0..200 {
        if harness.agent().is_streaming() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        harness.agent().is_streaming(),
        "parent agent must be streaming before we fire the trigger",
    );

    // Fire the trigger while parent is still mid-stream. The sub-agent is built with the
    // same stream_fn but its call is `n=1` so resolves immediately.
    let _ = executor
        .handle_trigger(sample_trigger("k-streaming", "trace-streaming"))
        .await;

    // Wait for `TriggerPromoted` so we know `apply_promotion` ran while parent was still
    // streaming. (Doesn't release the parent stream yet.)
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted { trace_id, .. } if trace_id == "trace-streaming" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    // The promotion ran during streaming → audit MUST be the queued shape, not success.
    // No Message::User in session yet — the loop hasn't drained the follow-up.
    let mid_entries = session.entries().await.unwrap();
    let mid_promotion_audit = mid_entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit must exist during streaming case");
    assert_eq!(
        mid_promotion_audit["state"], "queued",
        "streaming-branch promotion audit must report state=queued, got {mid_promotion_audit}",
    );
    assert!(
        mid_promotion_audit["inserted_entry_id"].is_null(),
        "inserted_entry_id MUST be null while message is queued (ID only known after loop drains)",
    );
    let mid_user_count = mid_entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                SessionTreeEntry::Message {
                    message: AgentMessage::Llm(theway_llm_provider::Message::User(_)),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        mid_user_count, 1,
        "before parent stream releases, session should have exactly 1 user message (the parent's initial prompt); got {mid_user_count}",
    );

    // Release the parent's first stream → loop appends assistant response → drains
    // follow_up → emits the promoted user message → session listener writes once.
    // Subsequent stream calls (parent's continuation after follow_up drain) resolve
    // immediately via `n != 0` branch.
    release.notify_one();
    let _ = parent_task.await.expect("parent task should join");
    // Allow listener writes to flush (subscribe_harness uses spawned tasks).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let final_entries = session.entries().await.unwrap();

    // Single persistence path: exactly TWO user messages now (initial prompt + promoted),
    // never three or more.
    let user_msgs: Vec<&SessionTreeEntry> = final_entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                SessionTreeEntry::Message {
                    message: AgentMessage::Llm(theway_llm_provider::Message::User(_)),
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        user_msgs.len(),
        2,
        "single persistence path: expected exactly 2 user messages (initial prompt + promoted), got {}",
        user_msgs.len(),
    );

    // Deterministic order: the promoted user message MUST come AFTER the parent's
    // assistant response in the session JSONL.
    let positions: Vec<(usize, &str)> = final_entries
        .iter()
        .enumerate()
        .filter_map(|(idx, e)| match e {
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } => match &u.content {
                theway_llm_provider::UserContent::Text(t) if t.starts_with("[Trigger ") => {
                    Some((idx, "promoted"))
                }
                _ => None,
            },
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
                ..
            } => Some((idx, "assistant")),
            _ => None,
        })
        .collect();
    let assistant_idx = positions
        .iter()
        .find(|(_, k)| *k == "assistant")
        .map(|(i, _)| *i);
    let promoted_idx = positions
        .iter()
        .find(|(_, k)| *k == "promoted")
        .map(|(i, _)| *i);
    assert!(
        assistant_idx.is_some() && promoted_idx.is_some(),
        "both assistant response and promoted user message must be persisted: {positions:?}",
    );
    assert!(
        promoted_idx.unwrap() > assistant_idx.unwrap(),
        "promoted user message MUST come AFTER the in-flight assistant response in session JSONL; got positions {positions:?}",
    );
}
