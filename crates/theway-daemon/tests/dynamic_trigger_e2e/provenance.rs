//! Trigger provenance: every engine-injected `[Trigger <trace_id>] ` turn is logged as a
//! canonical `Trigger` record immediately before the `Message::User` it produced. The record
//! carries the body with the engine prefix stripped, so the display projection shows the
//! source's own text while the model-facing message keeps the prefix.
//!
//! Both direct-inject deliveries are covered: `InjectSummary` (the promotion success path in
//! `apply_promotion`) and `InjectAndRun` (`run_trigger_action`), on the idle branch (both
//! entries appended synchronously) and on the streaming branch (both entries queued as
//! follow-ups and drained in order by the in-flight loop).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use theway_contract::user_input::{InputSource, UserInput};
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session, SessionStorage,
    SessionTreeEntry, StreamFn,
};
use theway_daemon::trigger_engine::event::TriggerEvent;
use theway_daemon::trigger_engine::execution::{BeforeTriggerActionHook, TriggerExecutor};
use theway_daemon::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, Trigger, TriggerAuthority,
    TriggerSource,
};
use theway_llm_provider::{
    AssistantMessageEvent, AssistantMessageEventStream, DoneReason, Message, UserContent,
};

use super::helpers::*;
use super::triggers;

/// Polling budget for every event and log assertion.
const WAIT: Duration = Duration::from_secs(5);

/// An MCP notification carrying `summary` — the source shape the direct-inject hooks match.
fn mcp_trigger(trace_id: &str, summary: &str) -> Trigger {
    Trigger {
        source: TriggerSource::Mcp {
            server_name: "notify".into(),
            method: "notify".into(),
        },
        source_kind: SourceKind::Mcp,
        source_label: "mcp:notify".into(),
        event_label: "build finished".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: Some(summary.to_string()),
        payload: None,
        idempotency_key: format!("provenance-{trace_id}"),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: trace_id.to_string(),
        authority: TriggerAuthority {
            principal_id: "e2e".into(),
            principal_label: "e2e".into(),
            credential_scope: CredentialScope::User,
            allowed_source_actions: vec![],
            expires_at: None,
        },
        received_at: chrono::Utc::now(),
    }
}

/// The daemon's direct-inject hook for a `notify` MCP server: `summary` routes its triggers
/// into `InjectSummary`, `run` into `InjectAndRun`.
fn direct_inject_hook(summary: bool, run: bool) -> BeforeTriggerActionHook {
    let mut summary_servers = HashSet::new();
    let mut run_servers = HashSet::new();
    if summary {
        summary_servers.insert("notify".to_string());
    }
    if run {
        run_servers.insert("notify".to_string());
    }
    let inner =
        triggers::before_trigger_action_hook(triggers::dynamic::DynamicTriggerRegistry::new());
    triggers::direct_inject_action_hook(summary_servers, run_servers, inner)
}

/// An executor over `harness`. Neither delivery under test calls a model, so no stream
/// function is wired.
fn executor_with(
    harness: &Arc<AgentHarness>,
    hook: Option<BeforeTriggerActionHook>,
) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        hook,
        None,
        None,
        None,
    ))
}

/// Event log shared by the assertions below.
fn event_log() -> Arc<parking_lot::Mutex<Vec<TriggerEvent>>> {
    Arc::new(parking_lot::Mutex::new(Vec::new()))
}

/// Subscribe `sink` to `executor`; the returned guard unsubscribes on drop.
fn subscribe(
    executor: &Arc<TriggerExecutor>,
    sink: &Arc<parking_lot::Mutex<Vec<TriggerEvent>>>,
) -> Box<dyn FnOnce() + Send> {
    let sink = sink.clone();
    executor.subscribe(Arc::new(move |event| sink.lock().push(event)))
}

/// Whether the log holds a completed promotion for `trace_id`.
fn saw_promoted(events: &Arc<parking_lot::Mutex<Vec<TriggerEvent>>>, trace_id: &str) -> bool {
    saw_event(events, trace_id, &is_promoted)
}

/// Whether the log holds an embedder run request for `trace_id`.
fn saw_main_run_request(
    events: &Arc<parking_lot::Mutex<Vec<TriggerEvent>>>,
    trace_id: &str,
) -> bool {
    saw_event(events, trace_id, &is_main_run_request)
}

/// Whether the log holds an event for `trace_id` that `predicate` accepts.
fn saw_event(
    events: &Arc<parking_lot::Mutex<Vec<TriggerEvent>>>,
    trace_id: &str,
    predicate: &dyn Fn(&TriggerEvent) -> bool,
) -> bool {
    let log = events.lock();
    log.iter()
        .any(|event| predicate(event) && event_trace_id(event) == Some(trace_id))
}

/// The promotion success event.
fn is_promoted(event: &TriggerEvent) -> bool {
    matches!(event, TriggerEvent::TriggerPromoted { .. })
}

/// The event requesting one parent run for an idle inject-and-run.
fn is_main_run_request(event: &TriggerEvent) -> bool {
    matches!(event, TriggerEvent::TriggerRequestsMainRun { .. })
}

/// The trace id an event carries, when it carries one.
fn event_trace_id(event: &TriggerEvent) -> Option<&str> {
    match event {
        TriggerEvent::TriggerPromoted { trace_id, .. } => Some(trace_id.as_str()),
        TriggerEvent::TriggerRequestsMainRun { trace_id } => Some(trace_id.as_str()),
        TriggerEvent::TriggerCompleted { trace_id, .. } => Some(trace_id.as_str()),
        _ => None,
    }
}

/// Poll `wanted` until it holds, with the [`WAIT`] deadline.
async fn wait_until(wanted: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    loop {
        if wanted() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The canonical `Trigger` record for `trace_id`, with its index in the session log.
fn trigger_record(entries: &[SessionTreeEntry], trace_id: &str) -> Option<(usize, UserInput)> {
    for (index, entry) in entries.iter().enumerate() {
        let SessionTreeEntry::Message { message, .. } = entry else {
            continue;
        };
        let AgentMessage::Custom(custom) = message else {
            continue;
        };
        if custom.role != UserInput::CUSTOM_ROLE {
            continue;
        }
        let Ok(record) = serde_json::from_value::<UserInput>(custom.payload.clone()) else {
            continue;
        };
        let is_trigger = record.source == InputSource::Trigger;
        let is_trace = record.source_ref.as_deref() == Some(trace_id);
        if is_trigger && is_trace {
            return Some((index, record));
        }
    }
    None
}

/// The text of a stored `Message::User` entry.
fn user_message_text(entry: &SessionTreeEntry) -> Option<String> {
    let SessionTreeEntry::Message { message, .. } = entry else {
        return None;
    };
    let AgentMessage::Llm(Message::User(user)) = message else {
        return None;
    };
    match &user.content {
        UserContent::Text(text) => Some(text.clone()),
        UserContent::Blocks(_) => None,
    }
}

/// Whether the log holds any assistant message (the kernel must not run the model itself).
fn has_assistant_message(entries: &[SessionTreeEntry]) -> bool {
    entries.iter().any(|entry| {
        let SessionTreeEntry::Message { message, .. } = entry else {
            return false;
        };
        matches!(message, AgentMessage::Llm(Message::Assistant(_)))
    })
}

/// Assert the log holds `trace_id`'s record immediately followed by the user message the
/// record describes, and return the record.
fn assert_record_then_message(
    entries: &[SessionTreeEntry],
    trace_id: &str,
    expected_body: &str,
) -> UserInput {
    let Some((index, record)) = trigger_record(entries, trace_id) else {
        panic!("no Trigger record for {trace_id}: {entries:#?}");
    };
    let body = entries.get(index + 1).and_then(user_message_text);
    assert_eq!(
        body.as_deref(),
        Some(expected_body),
        "entry {} must be the user message the record describes: {entries:#?}",
        index + 1
    );
    record
}

/// Poll the log until `trace_id`'s record and the message after it are both persisted, then
/// assert the pair. Polling both at once keeps the assertion out of the two sequential writes
/// of one follow-up drain.
async fn await_record_then_message(
    session: &Session,
    trace_id: &str,
    expected_body: &str,
) -> UserInput {
    let deadline = Instant::now() + WAIT;
    loop {
        let entries = session.entries().await.expect("session entries");
        if trigger_record(&entries, trace_id).is_some() {
            return assert_record_then_message(&entries, trace_id, expected_body);
        }
        assert!(
            Instant::now() < deadline,
            "no Trigger record for {trace_id} within {:?}: {entries:#?}",
            WAIT
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn promotion_logs_a_trigger_record_immediately_before_the_user_message() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.stream_fn = Some(dynamic_trigger_stream());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = executor_with(&harness, Some(direct_inject_hook(true, false)));
    let events = event_log();
    let _unsub = subscribe(&executor, &events);

    let trace_id = "trace-provenance-promote";
    let trigger = mcp_trigger(trace_id, "build finished successfully");
    let _ = executor.handle_trigger(trigger).await;
    let promoted = wait_until(|| saw_promoted(&events, trace_id)).await;
    assert!(promoted, "inject-summary promotion must complete");

    let entries = session.entries().await.expect("session entries");
    let expected = "[Trigger trace-provenance-promote] build finished successfully";
    let record = assert_record_then_message(&entries, trace_id, expected);

    // The record keeps the source's own text: no engine prefix, no parts.
    assert_eq!(record.text, "build finished successfully");
    assert_eq!(record.source, InputSource::Trigger);
    assert_eq!(record.source_ref.as_deref(), Some(trace_id));
    assert!(record.parts.is_empty(), "{:?}", record.parts);
}

#[tokio::test(flavor = "current_thread")]
async fn inject_and_run_logs_a_trigger_record_immediately_before_the_user_message() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.stream_fn = Some(dynamic_trigger_stream());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = executor_with(&harness, Some(direct_inject_hook(false, true)));
    let events = event_log();
    let _unsub = subscribe(&executor, &events);

    let trace_id = "trace-provenance-run";
    let trigger = mcp_trigger(trace_id, "deploy finished successfully");
    let _ = executor.handle_trigger(trigger).await;
    let requested = wait_until(|| saw_main_run_request(&events, trace_id)).await;
    assert!(requested, "idle inject-and-run must request the parent run");

    let entries = session.entries().await.expect("session entries");
    let expected = "[Trigger trace-provenance-run] deploy finished successfully";
    let record = assert_record_then_message(&entries, trace_id, expected);

    assert_eq!(record.text, "deploy finished successfully");
    assert_eq!(record.source, InputSource::Trigger);
    assert_eq!(record.source_ref.as_deref(), Some(trace_id));
    assert!(record.parts.is_empty(), "{:?}", record.parts);

    // The kernel still only requests the turn: no assistant message, no model run.
    assert!(
        !has_assistant_message(&entries),
        "the idle inject path must not run the model: {entries:#?}"
    );
}

/// A stream whose first call blocks until `release` is notified, so the test can fire a
/// trigger while the parent is mid-stream; later calls answer immediately.
fn controllable_stream_fn(release: Arc<tokio::sync::Notify>) -> StreamFn {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    Arc::new(move |_, _, _| {
        let call = counter.fetch_add(1, Ordering::SeqCst);
        let release = release.clone();
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            if call == 0 {
                release.notified().await;
            }
            let text = if call == 0 {
                "parent response"
            } else {
                "auxiliary response"
            };
            let message = assistant_text(text);
            sender.push(AssistantMessageEvent::Start {
                partial: message.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            });
        });
        stream
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_promotion_queues_the_record_before_the_user_message() {
    let _guard = DYNAMIC_TRIGGER_LOCK.lock().unwrap();
    triggers::global_registry().clear_for_tests();

    let release = Arc::new(tokio::sync::Notify::new());
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.on_control_plane_prompt = Some(allow_all_control_plane_hook());
    opts.stream_fn = Some(controllable_stream_fn(release.clone()));
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = executor_with(&harness, Some(direct_inject_hook(true, false)));
    let events = event_log();
    let _unsub = subscribe(&executor, &events);

    let parent = harness.clone();
    let parent_task = tokio::spawn(async move { parent.prompt("kick off parent").await });
    let streaming = wait_until(|| harness.agent().is_streaming()).await;
    assert!(
        streaming,
        "parent agent must be streaming before the trigger fires"
    );

    let trace_id = "trace-provenance-stream";
    let trigger = mcp_trigger(trace_id, "build finished mid-flight");
    let _ = executor.handle_trigger(trigger).await;
    let promoted = wait_until(|| saw_promoted(&events, trace_id)).await;
    assert!(
        promoted,
        "promotion must run while the parent is still streaming"
    );

    // The in-flight stream still holds the loop, so nothing may be persisted yet: the record
    // and the message wait in the follow-up queue together.
    let mid_entries = session.entries().await.expect("session entries");
    assert!(
        trigger_record(&mid_entries, trace_id).is_none(),
        "the queued record must not be persisted before the loop drains: {mid_entries:#?}"
    );

    release.notify_one();
    let expected = "[Trigger trace-provenance-stream] build finished mid-flight";
    let record = await_record_then_message(&session, trace_id, expected).await;

    assert_eq!(record.text, "build finished mid-flight");
    assert_eq!(record.source, InputSource::Trigger);
    assert_eq!(record.source_ref.as_deref(), Some(trace_id));

    // Bounded drain: the loop answers the drained message and ends its turn.
    let drain = tokio::time::timeout(WAIT, parent_task).await;
    assert!(
        drain.is_ok(),
        "the parent loop must drain its follow-ups and finish"
    );
}
