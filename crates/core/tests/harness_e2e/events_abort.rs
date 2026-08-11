//! Harness event bus, abort, budget/cost tracking, and listener isolation tests.

use super::helpers::{faux_model, faux_stream_fn};
use super::*;

/// Subscribing to the harness event bus must surface SessionStart on first prompt and Branch
/// on move_to. SessionStart is exactly-once over the harness lifetime.
#[tokio::test]
async fn harness_event_bus_delivers_session_and_branch() {
    use parking_lot::Mutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("ack"));
    let harness = AgentHarness::new(opts);

    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| {
        r2.lock().push(ev);
    });
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("hello").await.unwrap();
    harness.move_to(None, None).await.unwrap();

    let events = received.lock().clone();
    let kinds: Vec<&'static str> = events
        .iter()
        .map(|e| match e {
            HarnessEvent::SessionStart { .. } => "SessionStart",
            HarnessEvent::Compaction { .. } => "Compaction",
            HarnessEvent::Branch { .. } => "Branch",
            HarnessEvent::PersistenceError { .. } => "PersistenceError",
            HarnessEvent::TurnEnded { .. } => "TurnEnded",
            HarnessEvent::SkillsReloaded { .. } => "SkillsReloaded",
        })
        .collect();
    assert!(
        kinds.contains(&"SessionStart"),
        "expected SessionStart in {kinds:?}"
    );
    assert!(kinds.contains(&"Branch"), "expected Branch in {kinds:?}");

    harness.prompt("again").await.unwrap();
    let count_after = received
        .lock()
        .iter()
        .filter(|e| matches!(e, HarnessEvent::SessionStart { .. }))
        .count();
    assert_eq!(
        count_after, 1,
        "SessionStart must be exactly-once over the lifetime of a harness"
    );
}

/// Budget cap (issue #7): once the running cost crosses the configured USD cap, the next
/// prompt is rejected with a clear error before any LLM call is dispatched.
#[tokio::test]
async fn budget_cap_blocks_new_prompts_after_cap_reached() {
    use theway_llm_provider::UsageCost;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    // Deterministic usage that exceeds a $0.05 cap on the first turn.
    let usage = Usage {
        input: 10,
        output: 5,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 15,
        cost: UsageCost {
            input: 0.04,
            output: 0.02,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.06,
        },
    };
    let stream: StreamFn = {
        let usage = usage.clone();
        Arc::new(move |_, _, _| {
            let usage = usage.clone();
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text("ok")],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage,
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
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(stream);
    opts.budget_cap_usd = Some(0.05);
    let harness = AgentHarness::new(opts);

    // First prompt succeeds; cost crosses the cap in this turn.
    harness.prompt("one").await.unwrap();
    let snap = harness.cost();
    assert!(snap.tokens.cost.total >= 0.05, "cost should be >= cap");

    // Second prompt is rejected at the gate, with a useful message.
    let err = harness.prompt("two").await.unwrap_err().to_string();
    assert!(err.contains("budget cap reached"), "{err}");

    // Resetting the cost tracker unblocks the next prompt.
    harness.reset_cost();
    harness.prompt("three").await.unwrap();
}

/// Regression test for c4pt0r/theway#18. Prior behaviour: `Agent::abort()` cancelled the token
/// but `run_loop` only re-checked it after `stream.next()` returned, so an LLM stream that
/// stalled mid-flight kept the prompt future blocked. The fix races `stream.next()` against
/// `cancel.cancelled()` with a `biased` select.
///
/// This test uses a "never-emits" stream: the spawned task pushes nothing and parks itself.
/// Before the fix, `harness.abort()` would not unblock the prompt — the test would hang and
/// trigger the tokio test timeout. With the fix, the abort lands in <100ms.
#[tokio::test(flavor = "current_thread")]
async fn abort_promptly_unblocks_a_stalled_stream() {
    let stalled: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        // Keep the sender alive inside a parked task so `stream.next()` never resolves on its
        // own; only abort can unblock the consumer.
        tokio::spawn(async move {
            let _sender = sender; // hold ownership
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        stream
    });

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(stalled);
    let harness = Arc::new(AgentHarness::new(opts));

    let h2 = harness.clone();
    let prompt_task = tokio::spawn(async move { h2.prompt("hi").await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let abort_at = std::time::Instant::now();
    harness.abort();

    // The prompt future must resolve quickly after the abort signal. Anything beyond a
    // generous bound here means cancellation isn't being honored mid-stream.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), prompt_task)
        .await
        .expect("prompt task must resolve within 2s of abort")
        .expect("prompt task did not panic");
    let elapsed = abort_at.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "abort took {elapsed:?} — should be near-instant"
    );
    let err = outcome.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("abort"),
        "expected abort error: {err}"
    );
}

/// The harness's CostTracker accumulates Usage from every assistant turn. Two faux turns
/// with non-zero usage should produce a snapshot whose totals are the sum.
#[tokio::test]
async fn cost_tracker_accumulates_across_turns() {
    use theway_llm_provider::UsageCost;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    // Custom stream_fn that returns a deterministic Usage on every turn.
    let usage_per_turn = Usage {
        input: 25,
        output: 7,
        cache_read: 3,
        cache_write: 0,
        total_tokens: 35,
        cost: UsageCost {
            input: 0.01,
            output: 0.02,
            cache_read: 0.001,
            cache_write: 0.0,
            total: 0.031,
        },
    };
    let stream: StreamFn = {
        let usage = usage_per_turn.clone();
        Arc::new(move |_, _, _| {
            let usage = usage.clone();
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text("ok")],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage,
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

    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(stream);
    let harness = AgentHarness::new(opts);

    harness.prompt("one").await.unwrap();
    harness.prompt("two").await.unwrap();

    let s = harness.cost();
    assert_eq!(s.turn_count, 2);
    assert_eq!(s.tokens.input, 50);
    assert_eq!(s.tokens.output, 14);
    assert_eq!(s.tokens.cache_read, 6);
    assert_eq!(s.tokens.total_tokens, 70);
    assert!((s.tokens.cost.total - 0.062).abs() < 1e-9);

    harness.reset_cost();
    assert_eq!(harness.cost().turn_count, 0);
    assert_eq!(harness.cost().tokens.input, 0);
}

/// `Agent::abort` cancels the in-flight prompt cleanly: the prompt future resolves with an
/// `Err` and the session jsonl contains a user message (before the abort) but no further
/// assistant content for the cancelled turn.
#[tokio::test]
async fn abort_cancels_in_flight_prompt() {
    // A stream_fn that delays before emitting Done. The cancel token flip during this delay
    // should land us in the agent loop's abort branch.
    let slow_stream: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            // Long enough that the test has time to call abort() before Done arrives.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("should-not-arrive")],
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
    });

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(slow_stream);
    let harness = Arc::new(AgentHarness::new(opts));

    let h2 = harness.clone();
    let prompt_task = tokio::spawn(async move { h2.prompt("hi").await });

    // Give the agent loop a moment to install the cancel token.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    harness.abort();

    let outcome = prompt_task.await.expect("prompt task did not panic");
    assert!(outcome.is_err(), "aborted prompt should return Err");
    let err = outcome.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("abort"),
        "error should mention abort: {err}"
    );

    // Session should contain the user message we sent, but the slow assistant message must
    // NOT have been persisted (Done never reached MessageEnd before abort).
    let entries = session.entries().await.unwrap();
    let user_count = entries
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
    assert_eq!(user_count, 1, "user message should be persisted");
    let assistant_count = entries
        .iter()
        .filter(|e| {
            matches!(
                e,
                SessionTreeEntry::Message {
                    message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        assistant_count, 0,
        "no assistant turn should land on the aborted branch"
    );
}

/// A panicking listener does not poison the bus — other listeners still receive events.
#[tokio::test]
async fn harness_event_bus_isolates_panicking_listener() {
    use parking_lot::Mutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(faux_stream_fn("ack"));
    let harness = AgentHarness::new(opts);

    let received: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let r2 = received.clone();
    let good: HarnessListener = Arc::new(move |_ev| {
        *r2.lock() += 1;
    });
    let _unsub_good = harness.subscribe_harness(good);
    let _unsub_bad = harness.subscribe_harness(Arc::new(|_ev| panic!("isolated")));

    harness.prompt("hi").await.unwrap();
    harness.move_to(None, None).await.unwrap();

    assert!(
        *received.lock() >= 2,
        "good listener should still receive events past a panicking sibling"
    );
}

/// `subscribe_harness` returns an unsubscriber; after dropping it, the listener stops receiving.
#[tokio::test]
async fn subscribe_harness_unsub_stops_delivery() {
    use parking_lot::Mutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(opts);

    let count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let c2 = count.clone();
    let listener: HarnessListener = Arc::new(move |_ev| {
        *c2.lock() += 1;
    });
    let unsub = harness.subscribe_harness(listener);

    harness.prompt("first").await.unwrap();
    let before = *count.lock();
    assert!(before > 0, "listener should have received SessionStart");

    unsub();
    harness.prompt("second").await.unwrap();
    assert_eq!(
        *count.lock(),
        before,
        "no events should reach the listener after unsubscribe"
    );
}
