//! End-to-end AgentHarness test. Wires Agent + Session + a synthetic StreamFn and verifies the
//! prompt → assistant → session-persist cycle.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, CompactionSettings, HarnessEvent,
    HarnessListener, JsonlSessionRepo, MemorySessionStorage, Session, SessionError,
    SessionErrorCode, SessionStorage, SessionTreeEntry, Skill, SkillSource, StreamFn,
    ThinkingLevel, build_session_context,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream_fn(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
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

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        },
    ))
}

#[tokio::test]
async fn prompt_persists_user_and_assistant_to_session() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.system_prompt = "You are helpful.".into();
    opts.stream_fn = Some(faux_stream_fn("hello world"));
    let harness = AgentHarness::new(opts);

    assert!(harness.system_prompt().starts_with("You are helpful."));
    harness.prompt("hi").await.unwrap();

    let entries = session.entries().await.unwrap();
    // Should contain: user message + assistant message (both AgentMessage::Llm).
    assert!(
        entries.len() >= 2,
        "expected at least 2 entries, got {}",
        entries.len()
    );
    let has_assistant = entries.iter().any(|e| {
        matches!(
            e,
            theway_core::SessionTreeEntry::Message {
                message: theway_core::AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
                ..
            }
        )
    });
    assert!(has_assistant);
}

#[tokio::test]
async fn prompt_reports_session_persistence_failures() {
    struct FailingAppendStorage;

    #[async_trait::async_trait]
    impl SessionStorage for FailingAppendStorage {
        async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
            Ok(serde_json::json!({"id": "fail", "createdAt": "now"}))
        }
        async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
            Ok(None)
        }
        async fn set_leaf_id(&self, _id: Option<String>) -> Result<(), SessionError> {
            Ok(())
        }
        async fn create_entry_id(&self) -> Result<String, SessionError> {
            Ok("entry".into())
        }
        async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
            Err(SessionError {
                code: SessionErrorCode::StorageFailure,
                message: "disk full".into(),
            })
        }
        async fn get_entry(&self, _id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
            Ok(None)
        }
        async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
            Ok(Vec::new())
        }
        async fn get_path_to_root(
            &self,
            _leaf_id: Option<&str>,
        ) -> Result<Vec<SessionTreeEntry>, SessionError> {
            Ok(Vec::new())
        }
        async fn find_entries(
            &self,
            _entry_type: &str,
        ) -> Result<Vec<SessionTreeEntry>, SessionError> {
            Ok(Vec::new())
        }
        async fn get_label(&self, _id: &str) -> Result<Option<String>, SessionError> {
            Ok(None)
        }
    }

    let session = Session::new(Arc::new(FailingAppendStorage) as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(opts);

    let err = harness.prompt("hi").await.unwrap_err().to_string();
    assert!(err.contains("session append message"));
    assert!(err.contains("disk full"));
}

#[tokio::test]
async fn move_to_rehydrates_thinking_level_from_session_context() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    session.append_thinking_level_change("high").await.unwrap();
    let msg_id = session.append_message(user_message("hi")).await.unwrap();

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.thinking_level = ThinkingLevel::Off;
    opts.stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(opts);

    harness.move_to(Some(&msg_id), None).await.unwrap();

    assert_eq!(
        harness.agent().state().thinking_level,
        Some(ThinkingLevel::High)
    );
}

#[tokio::test]
async fn skills_block_appears_in_system_prompt() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    let skill = Skill {
        name: "my-skill".into(),
        description: "does things".into(),
        file_path: "/skills/my-skill/SKILL.md".into(),
        content: "the body".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.system_prompt = "Base.".into();
    opts.thinking_level = ThinkingLevel::Medium;
    opts.skills = vec![skill];
    opts.stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(opts);

    let prompt = harness.system_prompt();
    assert!(prompt.starts_with("Base."));
    assert!(prompt.contains("<skills>"));
    assert!(prompt.contains("- name: my-skill"));
}

#[tokio::test]
async fn set_model_persists_to_session() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    let model_a = faux_model();
    let mut opts = AgentHarnessOptions::new(model_a.clone(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(opts);

    let mut model_b = faux_model();
    model_b.id = "faux-v2".into();
    harness.set_model(model_b.clone()).await.unwrap();
    harness
        .set_thinking_level(theway_core::ThinkingLevel::Medium)
        .await
        .unwrap();

    let entries = session.entries().await.unwrap();
    assert!(entries.iter().any(|e| matches!(e,
        theway_core::SessionTreeEntry::ModelChange { model_id, .. } if model_id == "faux-v2"
    )));
    assert!(entries.iter().any(|e| matches!(e,
        theway_core::SessionTreeEntry::ThinkingLevelChange { thinking_level, .. } if thinking_level == "medium"
    )));
}

#[tokio::test]
async fn prompt_from_template_interpolates_and_runs() {
    use theway_core::PromptTemplate;
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("template-resp"));
    opts.prompt_templates = vec![PromptTemplate {
        name: "greet".into(),
        description: None,
        content: "Say hi to {{name}}".into(),
        file_path: "/tpl/greet.md".into(),
    }];
    let harness = AgentHarness::new(opts);

    let mut vars = serde_json::Map::new();
    vars.insert("name".into(), serde_json::json!("world"));
    harness.prompt_from_template("greet", vars).await.unwrap();

    // First persisted user message should have the interpolated text.
    let entries = session.entries().await.unwrap();
    let has_interpolated = entries.iter().any(|e| match e {
        theway_core::SessionTreeEntry::Message {
            message: theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(u)),
            ..
        } => matches!(&u.content, theway_llm_provider::UserContent::Text(s) if s == "Say hi to world"),
        _ => false,
    });
    assert!(
        has_interpolated,
        "expected interpolated user message; entries={:#?}",
        entries
    );
}

#[tokio::test]
async fn rehydrate_from_session_restores_messages_model_thinking() {
    use theway_core::{AgentMessage, ThinkingLevel};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    // Seed the session with a thinking-level change, a model change, and one user message —
    // simulating an earlier session the next harness is meant to pick up.
    session.append_thinking_level_change("high").await.unwrap();
    session.append_model_change("faux", "faux").await.unwrap();
    session
        .append_message(AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("earlier user prompt".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();

    // Build a harness whose initial state has *neither* the seeded model nor the high thinking
    // level — rehydrate must overwrite both.
    let cold_model = faux_model();
    let mut opts = AgentHarnessOptions::new(cold_model.clone(), session.clone());
    opts.thinking_level = ThinkingLevel::Off;
    opts.stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(opts);

    let ctx = harness.rehydrate_from_session().await.unwrap();
    assert_eq!(ctx.thinking_level, "high");
    assert_eq!(ctx.model.as_ref().unwrap().model_id, "faux");

    let state = harness.agent().state();
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.thinking_level, Some(ThinkingLevel::High));
    // Model is restored only when the catalog has the (provider, id) pair. The faux model is
    // not in the catalog, so we just check the API didn't blow away the cold-start model.
    assert!(state.model.is_some());
}

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
/// but `agent_loop` only re-checked it after `stream.next()` returned, so an LLM stream that
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

// ──────────────────────────────────────────────────────────────────────────────────────────
// Issue #19 regression tests — compaction `first_kept_entry_id` must be reachable in the
// session jsonl so `--resume` reconstructs the kept tail.
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Round-trip: drive the harness through a few turns + force_compact, then drop the harness,
/// reopen the same session jsonl, and verify `build_session_context` reproduces what was in
/// in-memory state after compaction. The pre-fix bug was that `first_kept_entry_id` written to
/// the session jsonl referenced a synthetic id that no real entry carried, so the rebuilt
/// branch dropped the entire pre-compaction tail.
#[tokio::test]
async fn force_compact_writes_reachable_first_kept_entry_id_and_resume_preserves_tail() {
    let dir = tempfile::tempdir().unwrap();
    let repo = JsonlSessionRepo::new(dir.path());
    let session = repo.create("/tmp/test-cwd").await.unwrap();
    let session_files = repo.list().await.unwrap();
    assert_eq!(session_files.len(), 1);
    let session_path = session_files[0].clone();

    // Build a harness with a low keep_recent_tokens so a small transcript triggers compaction.
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("summary or assistant reply"));
    opts.compaction = CompactionSettings {
        enabled: true,
        reserve_tokens: 0,
        keep_recent_tokens: 4, // forces the cut close to the end
        algorithm: "builtin".into(),
    };
    let harness = AgentHarness::new(opts);

    // Drive three short prompts so we have ≥3 user/assistant pairs in the session.
    harness.prompt("first").await.unwrap();
    harness.prompt("second").await.unwrap();
    harness.prompt("third").await.unwrap();

    let entries_before = session.entries().await.unwrap();
    let pre_compact_msg_count = entries_before
        .iter()
        .filter(|e| matches!(e, SessionTreeEntry::Message { .. }))
        .count();
    assert!(
        pre_compact_msg_count >= 6,
        "expected at least 3 user+assistant pairs in session, got {pre_compact_msg_count}"
    );

    // Force compaction.
    let ran = harness.force_compact(None).await.unwrap();
    assert!(ran, "force_compact should have produced a summary");

    // Verify the persisted Compaction entry's first_kept_entry_id is reachable.
    let entries_after = session.entries().await.unwrap();
    let compaction_entry = entries_after
        .iter()
        .rev()
        .find(|e| matches!(e, SessionTreeEntry::Compaction { .. }))
        .expect("session should have a Compaction entry");
    let SessionTreeEntry::Compaction {
        first_kept_entry_id,
        ..
    } = compaction_entry
    else {
        unreachable!()
    };
    assert!(
        !first_kept_entry_id.is_empty(),
        "first_kept_entry_id must be set when compaction ran"
    );
    let kept = entries_after
        .iter()
        .find(|e| e.id() == first_kept_entry_id.as_str())
        .expect(
            "first_kept_entry_id MUST be reachable in the session entries (issue #19 regression)",
        );
    // The kept entry must be a `Message` and specifically a user-turn boundary.
    let kept_msg = match kept {
        SessionTreeEntry::Message { message, .. } => message,
        other => panic!(
            "first_kept_entry_id should point to a `Message` entry, got {:?}",
            other.type_str()
        ),
    };
    assert!(
        matches!(
            kept_msg,
            AgentMessage::Llm(theway_llm_provider::Message::User(_))
        ),
        "first_kept_entry_id should land on a user-turn-boundary Message"
    );

    // Snapshot in-memory state right after compaction.
    let in_memory_after = harness.agent().state().messages.clone();
    drop(harness);

    // Reopen the session from disk and rebuild the context.
    let reopened = repo.open(&session_path).await.unwrap();
    let branch = reopened.branch(None).await.unwrap();
    let rebuilt = build_session_context(&branch);

    // The rebuilt message list must be non-trivial (the bug dropped everything except the
    // summary) and must contain the same tail messages the live agent kept.
    assert!(
        rebuilt.messages.len() >= in_memory_after.len(),
        "rebuilt context lost messages (live={}, rebuilt={}) — pre-fix regression",
        in_memory_after.len(),
        rebuilt.messages.len(),
    );
    // First message in both should be the compaction summary.
    match (&in_memory_after[0], &rebuilt.messages[0]) {
        (AgentMessage::Custom(a), AgentMessage::Custom(b)) => {
            assert_eq!(a.role, "compaction_summary");
            assert_eq!(b.role, "compaction_summary");
        }
        _ => panic!("expected both in-memory and rebuilt to start with compaction_summary"),
    }
}

/// `build_session_context` must never inject `Custom { custom_type: "trigger" }` entries into
/// the LLM message stream — those are audit trail only. Adding this assertion now so the RFC 1
/// trigger work (issue #20) can rely on it as a prerequisite invariant.
#[tokio::test]
async fn build_session_context_skips_trigger_custom_entries() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    let id_user = session.append_message(user_message("hello")).await.unwrap();
    let _id_trigger = session
        .append_custom(
            "trigger",
            Some(serde_json::json!({ "trace_id": "trace-1", "source_kind": "Mcp" })),
        )
        .await
        .unwrap();
    let id_after = session
        .append_message(user_message("after trigger"))
        .await
        .unwrap();

    // The raw branch must include the trigger Custom entry (audit trail intact).
    let branch = session.branch(None).await.unwrap();
    let trigger_present = branch.iter().any(|e| {
        matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger"
        )
    });
    assert!(
        trigger_present,
        "session.branch must still enumerate trigger Custom entries (audit trail)"
    );
    assert_eq!(branch.len(), 3);

    // build_session_context must NOT translate the trigger Custom into an LLM message.
    let ctx = build_session_context(&branch);
    assert_eq!(
        ctx.messages.len(),
        2,
        "expected only the two user Message entries in the LLM stream"
    );
    let ids: Vec<&str> = branch
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Message { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![id_user.as_str(), id_after.as_str()]);
}

/// `find_cut_point` (and `find_turn_start_index`) must always anchor `first_kept_entry_id` on
/// a user-turn-boundary `Message` even when the cut threshold falls on or next to a trigger
/// `Custom` entry. RFC 1 prerequisite — agent state mapping/rehydrate becomes ambiguous if
/// `first_kept_entry_id` is allowed to reference a non-Message entry.
#[tokio::test]
async fn cut_point_anchors_on_user_message_even_around_trigger_custom() {
    use theway_core::find_cut_point;

    // Build entries: user → assistant → Custom(trigger) → user → assistant.
    // With keep_recent_tokens=1, the algorithm walks backward and hits the last
    // user message; verify it does not land on the trigger Custom.
    let user_a = SessionTreeEntry::Message {
        id: "msg-user-a".into(),
        parent_id: None,
        timestamp: "t".into(),
        message: user_message("user a"),
    };
    let assistant_a = SessionTreeEntry::Message {
        id: "msg-asst-a".into(),
        parent_id: Some("msg-user-a".into()),
        timestamp: "t".into(),
        message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("asst a")],
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
        })),
    };
    let trigger_custom = SessionTreeEntry::Custom {
        id: "custom-trigger-1".into(),
        parent_id: Some("msg-asst-a".into()),
        timestamp: "t".into(),
        custom_type: "trigger".into(),
        data: Some(serde_json::json!({"trace_id": "trace-1"})),
    };
    let user_b = SessionTreeEntry::Message {
        id: "msg-user-b".into(),
        parent_id: Some("custom-trigger-1".into()),
        timestamp: "t".into(),
        message: user_message("user b"),
    };
    let assistant_b = SessionTreeEntry::Message {
        id: "msg-asst-b".into(),
        parent_id: Some("msg-user-b".into()),
        timestamp: "t".into(),
        message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("asst b")],
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
        })),
    };
    let entries = vec![user_a, assistant_a, trigger_custom, user_b, assistant_b];

    let cut = find_cut_point(
        &entries,
        &CompactionSettings {
            enabled: true,
            reserve_tokens: 0,
            keep_recent_tokens: 1, // tiny: forces walk-back to nearest user message
            algorithm: "builtin".into(),
        },
    );

    let first_kept_id = cut
        .first_kept_entry_id
        .as_deref()
        .expect("non-empty entries must yield a first_kept_entry_id");
    let kept = entries
        .iter()
        .find(|e| e.id() == first_kept_id)
        .expect("first_kept_entry_id must be reachable in entries");
    // Crucial: must be a Message (not Custom), and the message must be a user turn boundary.
    match kept {
        SessionTreeEntry::Message { message, .. } => {
            assert!(
                matches!(
                    message,
                    AgentMessage::Llm(theway_llm_provider::Message::User(_))
                ),
                "first_kept_entry_id must land on a user-turn boundary Message"
            );
        }
        other => panic!(
            "first_kept_entry_id pointed to {:?}, expected Message",
            other.type_str()
        ),
    }
}

/// `session.branch(None)` failure during compaction must short-circuit cleanly: no
/// `Compaction` entry appended, no agent state mutation, no panic, and the harness emits a
/// diagnostic `HarnessEvent::Compaction` whose summary starts with `compaction skipped:` so
/// observers know why. This is the issue #19 acceptance item for runtime fallback.
#[tokio::test]
async fn force_compact_fallback_when_session_branch_read_fails() {
    use async_trait::async_trait;
    use parking_lot::Mutex as PlMutex;
    use serde_json::Value;
    use theway_core::SessionError;

    /// Wraps `MemorySessionStorage`; lets the test toggle `get_path_to_root` into an error
    /// state to simulate disk read failure mid-compaction.
    struct FailingBranchStorage {
        inner: MemorySessionStorage,
        fail_branch: PlMutex<bool>,
    }

    impl FailingBranchStorage {
        fn new() -> Self {
            Self {
                inner: MemorySessionStorage::new(),
                fail_branch: PlMutex::new(false),
            }
        }
        fn arm(&self) {
            *self.fail_branch.lock() = true;
        }
    }

    #[async_trait]
    impl SessionStorage for FailingBranchStorage {
        async fn get_metadata_json(&self) -> Result<Value, SessionError> {
            self.inner.get_metadata_json().await
        }
        async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
            self.inner.get_leaf_id().await
        }
        async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
            self.inner.set_leaf_id(id).await
        }
        async fn create_entry_id(&self) -> Result<String, SessionError> {
            self.inner.create_entry_id().await
        }
        async fn append_entry(&self, entry: SessionTreeEntry) -> Result<(), SessionError> {
            self.inner.append_entry(entry).await
        }
        async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
            self.inner.get_entry(id).await
        }
        async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
            self.inner.get_entries().await
        }
        async fn get_path_to_root(
            &self,
            leaf_id: Option<&str>,
        ) -> Result<Vec<SessionTreeEntry>, SessionError> {
            if *self.fail_branch.lock() {
                return Err(SessionError {
                    code: SessionErrorCode::StorageFailure,
                    message: "simulated branch read failure".into(),
                });
            }
            self.inner.get_path_to_root(leaf_id).await
        }
        async fn find_entries(
            &self,
            entry_type: &str,
        ) -> Result<Vec<SessionTreeEntry>, SessionError> {
            self.inner.find_entries(entry_type).await
        }
        async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
            self.inner.get_label(id).await
        }
    }

    let storage = Arc::new(FailingBranchStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("would-be summary"));
    opts.compaction = CompactionSettings {
        enabled: true,
        reserve_tokens: 0,
        keep_recent_tokens: 4,
        algorithm: "builtin".into(),
    };
    let harness = AgentHarness::new(opts);

    // Drive one normal prompt so we have a non-empty session before failure.
    harness.prompt("first").await.unwrap();
    let pre_entries = storage.inner.get_entries().await.unwrap();
    let pre_state_len = harness.agent().state().messages.len();

    // Collect HarnessEvent::Compaction emissions.
    let events: Arc<PlMutex<Vec<HarnessEvent>>> = Arc::new(PlMutex::new(Vec::new()));
    let events_clone = events.clone();
    let _unsub = harness.subscribe_harness(Arc::new(move |ev: HarnessEvent| {
        events_clone.lock().push(ev);
    }) as HarnessListener);

    // Arm the failure and force compaction. Must not panic, must return Ok(false).
    storage.arm();
    let ran = harness.force_compact(None).await.unwrap();
    assert!(
        !ran,
        "force_compact must return Ok(false) when session branch read fails"
    );

    // Session must NOT have a new Compaction entry.
    let post_entries = storage.inner.get_entries().await.unwrap();
    assert_eq!(
        post_entries.len(),
        pre_entries.len(),
        "session must not gain entries when compaction is aborted by branch read failure"
    );
    let added_compaction = post_entries[pre_entries.len()..]
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Compaction { .. }));
    assert!(
        !added_compaction,
        "no Compaction entry must be appended on branch read failure"
    );

    // Agent state must be unchanged (same message count, same prefix).
    assert_eq!(
        harness.agent().state().messages.len(),
        pre_state_len,
        "agent state.messages must not be mutated when compaction is aborted"
    );

    // A diagnostic Compaction event must have been emitted with the `compaction skipped:`
    // prefix so observers can tell why.
    let events_snapshot = events.lock().clone();
    let saw_diagnostic = events_snapshot.iter().any(|ev| match ev {
        HarnessEvent::Compaction {
            summary,
            tokens_before,
            ..
        } => summary.starts_with("compaction skipped:") && *tokens_before == 0,
        _ => false,
    });
    assert!(
        saw_diagnostic,
        "expected a diagnostic HarnessEvent::Compaction (summary starts with 'compaction skipped:') — events: {:?}",
        events_snapshot
    );
}

#[tokio::test]
async fn auto_compaction_bounds_oversized_summary_prompt_before_provider_call() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut model = faux_model();
    model.context_window = 5_000;

    let saw_compaction = Arc::new(AtomicBool::new(false));
    let saw_compaction_clone = saw_compaction.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, _| {
        let is_compaction = context
            .system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("context summarization assistant"));
        if is_compaction {
            let text = match &context.messages[0] {
                theway_llm_provider::Message::User(user) => match &user.content {
                    theway_llm_provider::UserContent::Text(text) => text.as_str(),
                    _ => "",
                },
                _ => "",
            };
            assert!(
                text.len().div_ceil(4) < 4_000,
                "auto-compaction must bound the summarizer prompt before provider dispatch; got {} chars",
                text.len()
            );
            assert!(
                text.contains("[compaction note: omitted"),
                "bounded summary prompt must disclose omitted content"
            );
            saw_compaction_clone.store(true, Ordering::SeqCst);
        }

        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(if is_compaction {
                    "bounded compaction summary"
                } else {
                    "normal assistant reply"
                })],
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
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });

    let mut opts = AgentHarnessOptions::new(model, session.clone());
    opts.stream_fn = Some(stream_fn);
    opts.compaction = CompactionSettings {
        enabled: true,
        reserve_tokens: 1_000,
        keep_recent_tokens: 1,
        algorithm: "builtin".into(),
    };
    let harness = AgentHarness::new(opts);

    for i in 0..80 {
        let message = user_message(&format!("old-msg-{i} {}", "x".repeat(1600)));
        session.append_message(message.clone()).await.unwrap();
        harness.agent().state().messages.push(message);
    }

    harness.prompt("next turn").await.unwrap();
    assert!(
        saw_compaction.load(Ordering::SeqCst),
        "oversized context should trigger bounded auto-compaction"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// handle_trigger — RFC 1 sub-PR 2 (moved to crates/server/tests/trigger_execution.rs)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn control_plane_write_category_defaults_to_allow_at_runtime_layer() {
    use theway_core::{PermissionCategory, PermissionDecision, PermissionPolicy};

    let policy = PermissionPolicy::default_for_coding_agent();
    // Even with bash-tool name + a normally-dangerous arg, the ControlPlaneWrite category
    // should fall through to Allow because the runtime policy has no category-specific
    // classifier wired. Tools-MCP's follow-up PR adds the danger classifier here.
    let args = serde_json::json!({ "command": "rm -rf /tmp/foo" });
    match policy.evaluate_with_category(PermissionCategory::ControlPlaneWrite, "bash", &args) {
        PermissionDecision::Allow => {}
        other => panic!("ControlPlaneWrite must default to Allow at runtime; got {other:?}"),
    }
    // Sanity check the legacy `evaluate` still uses the Tool category (bash classifier)
    // so backwards compatibility holds.
    match policy.evaluate("bash", &args) {
        PermissionDecision::Deny { .. } => {}
        other => panic!("Tool-category bash danger classifier must still deny; got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Skill catalog hot-reload (issue #87 sub-PR A — `reload_skills_from_disk` API).
//
// Pins the invariants every downstream consumer (`InstallSkillTool`, `/skills reload`,
// future Web UI) needs to trust:
//   - `reload_skills_from_disk` calls the embedder-supplied loader exactly once per call
//     and applies the result via `replace_skills` so the system prompt rebuilds.
//   - Loader diagnostics propagate to the caller so install tools can surface "loaded N
//     skills, M warnings" without parsing free-form text.
//   - When no loader is configured, the API errors with `NotConfigured` instead of
//     silently no-opping.
//   - Reload only swaps the skill catalog + system prompt — it never touches
//     `state.messages` or `is_streaming`. (In-flight turn isn't interrupted.)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reload_skills_from_disk_invokes_loader_and_replaces_catalog() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use theway_core::{LoadSkillsOutput, Skill};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.skills = vec![Skill {
        name: "original".into(),
        content: "original body".into(),
        description: "the skill we ship with".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
        file_path: "/tmp/original".into(),
    }];

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_for_loader = call_count.clone();
    opts.reload_skills_fn = Some(Arc::new(move || {
        let call_count = call_count_for_loader.clone();
        Box::pin(async move {
            call_count.fetch_add(1, Ordering::SeqCst);
            LoadSkillsOutput {
                skills: vec![
                    Skill {
                        name: "fresh-one".into(),
                        content: "after install".into(),
                        description: "newly installed".into(),
                        disable_model_invocation: false,
                        source: SkillSource::User,
                        file_path: "/tmp/fresh-one".into(),
                    },
                    Skill {
                        name: "fresh-two".into(),
                        content: "second new".into(),
                        description: "also newly installed".into(),
                        disable_model_invocation: false,
                        source: SkillSource::User,
                        file_path: "/tmp/fresh-two".into(),
                    },
                ],
                diagnostics: Vec::new(),
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    // Before reload: original catalog present.
    let before = harness.skills();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].name, "original");

    let result = harness
        .reload_skills_from_disk()
        .await
        .expect("loader configured");
    assert_eq!(result.skills.len(), 2);
    assert_eq!(call_count.load(Ordering::SeqCst), 1, "loader called once");

    // After reload: catalog replaced, system prompt rebuilt with new <skills> block.
    let after = harness.skills();
    assert_eq!(after.len(), 2);
    assert!(after.iter().any(|s| s.name == "fresh-one"));
    assert!(after.iter().any(|s| s.name == "fresh-two"));
    assert!(
        after.iter().all(|s| s.name != "original"),
        "old skill must be gone — single source of truth is the loader",
    );
    let prompt = harness.system_prompt();
    assert!(
        prompt.contains("fresh-one") && prompt.contains("fresh-two"),
        "system_prompt must rebuild with new <skills> block; got: {prompt}"
    );
    assert!(
        !prompt.contains("the skill we ship with"),
        "original skill description must not leak into rebuilt prompt: {prompt}",
    );
}

/// The UI sidebar repaints off harness events — a catalog hot-reload that emits nothing
/// leaves the skills panel stale (e.g. a sub-agent installing a skill while the parent
/// is idle). Every successful reload must announce itself.
#[tokio::test]
async fn reload_skills_from_disk_emits_skills_reloaded_event() {
    use std::sync::Mutex;
    use theway_core::{LoadSkillsOutput, Skill};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.reload_skills_fn = Some(Arc::new(move || {
        Box::pin(async move {
            LoadSkillsOutput {
                skills: vec![Skill {
                    name: "fresh-one".into(),
                    content: "after install".into(),
                    description: "newly installed".into(),
                    disable_model_invocation: false,
                    source: SkillSource::User,
                    file_path: "/tmp/fresh-one".into(),
                }],
                diagnostics: Vec::new(),
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let received_for_listener = received.clone();
    let _unsubscribe = harness.subscribe_harness(Arc::new(move |event| {
        received_for_listener.lock().unwrap().push(event);
    }));

    harness
        .reload_skills_from_disk()
        .await
        .expect("loader configured");

    let events = received.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HarnessEvent::SkillsReloaded { total: 1 })),
        "reload must emit SkillsReloaded with the new catalog size; got {} event(s)",
        events.len()
    );
}

#[tokio::test]
async fn reload_skills_from_disk_propagates_loader_diagnostics() {
    use theway_core::{LoadSkillsOutput, Skill, SkillDiagnostic, SkillDiagnosticCode};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.reload_skills_fn = Some(Arc::new(move || {
        Box::pin(async move {
            // Realistic mixed result: one good skill + one diagnostic for a bad one. Bad
            // skill doesn't block the good one — that's the existing `load_skills` policy
            // and the embedder relies on it.
            LoadSkillsOutput {
                skills: vec![Skill {
                    name: "good".into(),
                    content: "ok".into(),
                    description: "valid skill".into(),
                    disable_model_invocation: false,
                    source: SkillSource::User,
                    file_path: "/tmp/good".into(),
                }],
                diagnostics: vec![SkillDiagnostic {
                    code: SkillDiagnosticCode::ParseFailed,
                    message: "frontmatter malformed".into(),
                    path: "/tmp/bad/SKILL.md".into(),
                }],
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    let result = harness.reload_skills_from_disk().await.unwrap();

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.diagnostics.len(), 1);
    assert!(
        result.diagnostics[0]
            .message
            .contains("frontmatter malformed"),
        "diagnostic message must propagate verbatim to the install tool",
    );
    assert_eq!(harness.skills().len(), 1);
}

#[tokio::test]
async fn reload_skills_from_disk_without_loader_errors_with_not_configured() {
    use theway_core::ReloadSkillsError;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    // No reload_skills_fn — default None.
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));

    let err = harness
        .reload_skills_from_disk()
        .await
        .expect_err("loader missing should error, not silently no-op");
    assert!(
        matches!(err, ReloadSkillsError::NotConfigured),
        "expected NotConfigured, got {err:?}"
    );
}

#[tokio::test]
async fn reload_skills_from_disk_preserves_message_state_and_streaming_flag() {
    use theway_core::{LoadSkillsOutput, Skill};

    // Pin the "reload doesn't touch loop state" invariant: it only swaps the skill catalog
    // + system prompt. `state.messages` and `is_streaming` are the agent loop's
    // concerns; reload must not perturb them. Downstream consumers (InstallSkillTool
    // called from a sub-agent, `/skills reload` slash command) rely on this — they call
    // reload without coordinating with the parent loop.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("ok"));
    opts.reload_skills_fn = Some(Arc::new(move || {
        Box::pin(async move {
            LoadSkillsOutput {
                skills: vec![Skill {
                    name: "reloaded".into(),
                    content: "fresh".into(),
                    description: "post-reload".into(),
                    disable_model_invocation: false,
                    source: SkillSource::User,
                    file_path: "/tmp/reloaded".into(),
                }],
                diagnostics: Vec::new(),
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    // Drive one normal turn so state.messages has content + system_prompt is established.
    harness.prompt("hello").await.unwrap();
    let pre_messages_len = harness.agent().state().messages.len();
    assert!(
        pre_messages_len > 0,
        "expected at least one message after prompt"
    );
    let pre_is_streaming = harness.agent().is_streaming();

    let result = harness.reload_skills_from_disk().await.unwrap();
    assert_eq!(result.skills.len(), 1);

    let post_messages_len = harness.agent().state().messages.len();
    let post_is_streaming = harness.agent().is_streaming();
    assert_eq!(
        post_messages_len, pre_messages_len,
        "reload must not touch state.messages",
    );
    assert_eq!(
        post_is_streaming, pre_is_streaming,
        "reload must not touch is_streaming",
    );

    // The system_prompt DOES get the new <skills> block — that's the whole point.
    assert!(
        harness.system_prompt().contains("reloaded"),
        "rebuilt system_prompt should mention new skill",
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// OnTurnEndHook (powers `/goal` and other turn-completion driven orchestrators)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Per-call stream_fn that returns successive assistant texts from a shared queue.
/// Used to differentiate iterations in continuation-loop tests so we can assert which
/// turn produced which message.
fn faux_stream_fn_sequence(texts: Vec<&'static str>) -> StreamFn {
    let cursor = Arc::new(std::sync::Mutex::new(0usize));
    let texts = Arc::new(texts);
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let texts = texts.clone();
        let cursor = cursor.clone();
        tokio::spawn(async move {
            let idx = {
                let mut c = cursor.lock().unwrap();
                let i = *c;
                *c = c.saturating_add(1);
                i
            };
            let text = *texts.get(idx).unwrap_or(&"<exhausted>");
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
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

fn collect_turn_end_decisions(
    received: &Arc<parking_lot::Mutex<Vec<HarnessEvent>>>,
) -> Vec<(&'static str, u32, Option<String>)> {
    received
        .lock()
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::TurnEnded {
                decision,
                continuation_count,
                reason,
                ..
            } => Some((*decision, *continuation_count, reason.clone())),
            _ => None,
        })
        .collect()
}

async fn read_custom_entries(
    storage: Arc<MemorySessionStorage>,
    kind: &str,
) -> Vec<serde_json::Value> {
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let entries = session.branch(None).await.unwrap();
    entries
        .into_iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == kind => data,
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn on_turn_end_hook_unset_keeps_legacy_single_cycle_behavior() {
    use parking_lot::Mutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("only-turn"));
    // No on_turn_end set — legacy path.

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("hi").await.unwrap();

    let turn_end_count = received
        .lock()
        .iter()
        .filter(|e| matches!(e, HarnessEvent::TurnEnded { .. }))
        .count();
    assert_eq!(turn_end_count, 0, "no TurnEnded event when hook is unset",);

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert!(
        audits.is_empty(),
        "no turn_end_decision audit when hook is unset"
    );
}

#[tokio::test]
async fn on_turn_end_hook_noop_writes_no_audit_no_event() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("just answering"));
    // Hook is permanently registered (e.g. `/goal` always-on hook), but
    // returns Noop when there's no active goal — should look identical to
    // "no hook configured" from the session's point of view.
    let invocation_count = Arc::new(Mutex::new(0u32));
    let ic = invocation_count.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let ic = ic.clone();
        Box::pin(async move {
            *ic.lock() += 1;
            TurnEndDecision {
                action: TurnEndAction::Noop,
                payload: Some(serde_json::json!({ "ignored": true })),
            }
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("hi").await.unwrap();

    assert_eq!(
        *invocation_count.lock(),
        1,
        "hook fires exactly once per turn even when it returns Noop",
    );
    let turn_end_count = received
        .lock()
        .iter()
        .filter(|e| matches!(e, HarnessEvent::TurnEnded { .. }))
        .count();
    assert_eq!(turn_end_count, 0, "Noop emits no TurnEnded event");

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert!(audits.is_empty(), "Noop writes no turn_end_decision audit");
}

#[tokio::test]
async fn on_turn_end_hook_stop_emits_event_and_audits_payload() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndContext, OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let observed_ctx: Arc<Mutex<Option<OnTurnEndContext>>> = Arc::new(Mutex::new(None));
    let observed_ctx_for_hook = observed_ctx.clone();

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("first-and-only"));
    let hook: OnTurnEndHook = Arc::new(move |ctx, _cancel| {
        let observed = observed_ctx_for_hook.clone();
        Box::pin(async move {
            *observed.lock() = Some(ctx);
            TurnEndDecision {
                action: TurnEndAction::Stop,
                payload: Some(serde_json::json!({ "kind": "goal_achieved" })),
            }
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("what is 2+2?").await.unwrap();

    let decisions = collect_turn_end_decisions(&received);
    assert_eq!(
        decisions,
        vec![("stop", 0, None)],
        "exactly one stop event with continuation_count=0"
    );

    let ctx = observed_ctx.lock().take().unwrap();
    assert_eq!(ctx.continuation_count, 0);
    assert_eq!(
        ctx.last_user_prompt.as_deref(),
        Some("what is 2+2?"),
        "hook sees the originating user prompt"
    );
    assert!(
        !ctx.transcript.is_empty(),
        "hook sees a non-empty transcript snapshot"
    );

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert_eq!(audits.len(), 1, "exactly one audit entry");
    assert_eq!(audits[0]["decision"], "stop");
    assert_eq!(audits[0]["continuation_count"], 0);
    assert_eq!(audits[0]["payload"]["kind"], "goal_achieved");
}

#[tokio::test]
async fn on_turn_end_continue_runs_second_turn_then_stops() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_sequence(vec!["first-turn", "second-turn"]));
    let call_count = Arc::new(Mutex::new(0u32));
    let cc = call_count.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let cc = cc.clone();
        Box::pin(async move {
            let n = {
                let mut g = cc.lock();
                *g = g.saturating_add(1);
                *g
            };
            if n == 1 {
                TurnEndDecision {
                    action: TurnEndAction::Continue {
                        prompt: "now do step 2".into(),
                    },
                    payload: Some(serde_json::json!({ "iter": 1 })),
                }
            } else {
                TurnEndDecision {
                    action: TurnEndAction::Stop,
                    payload: None,
                }
            }
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("start").await.unwrap();

    let decisions = collect_turn_end_decisions(&received);
    assert_eq!(
        decisions,
        vec![("continue", 1, None), ("stop", 1, None),],
        "continue then stop, post-decision counts: 1 then 1",
    );

    // Two audit entries with matching payloads.
    let audits = read_custom_entries(storage.clone(), "turn_end_decision").await;
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0]["decision"], "continue");
    assert_eq!(audits[0]["next_prompt_preview"], "now do step 2");
    assert_eq!(audits[0]["payload"]["iter"], 1);
    assert_eq!(audits[1]["decision"], "stop");
    assert!(audits[1]["payload"].is_null());

    // The transcript should have the original user msg + assistant + continuation user msg + assistant.
    let agent_state_messages = {
        let s = harness.agent().state();
        s.messages
            .iter()
            .map(|m| match m {
                AgentMessage::Llm(theway_llm_provider::Message::User(u)) => match &u.content {
                    theway_llm_provider::UserContent::Text(t) => format!("user:{t}"),
                    theway_llm_provider::UserContent::Blocks(_) => "user:<blocks>".into(),
                },
                AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => {
                    let text: String = a
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    format!("assistant:{text}")
                }
                AgentMessage::Llm(_) => "other-llm".into(),
                AgentMessage::Custom(_) => "custom".into(),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        agent_state_messages,
        vec![
            "user:start",
            "assistant:first-turn",
            "user:now do step 2",
            "assistant:second-turn",
        ],
        "continuation appended the hook's prompt as a new user message"
    );
}

#[tokio::test]
async fn on_turn_end_continuation_cap_emits_budget_limited_without_invoking_hook() {
    use parking_lot::Mutex;
    use theway_core::{OnTurnEndHook, TurnEndAction, TurnEndDecision};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_sequence(vec![
        "t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8",
    ]));
    // Cap is 2: original turn + at most 2 continuations, then budget_limited.
    opts.turn_continuation_cap = Some(2);

    let hook_invocations = Arc::new(Mutex::new(0u32));
    let hi = hook_invocations.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let hi = hi.clone();
        Box::pin(async move {
            *hi.lock() += 1;
            TurnEndDecision::from(TurnEndAction::Continue {
                prompt: "keep going".into(),
            })
        })
    });
    opts.on_turn_end = Some(hook);

    let harness = AgentHarness::new(opts);
    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = received.clone();
    let listener: HarnessListener = Arc::new(move |ev| r2.lock().push(ev));
    let _unsub = harness.subscribe_harness(listener);

    harness.prompt("go").await.unwrap();

    let decisions = collect_turn_end_decisions(&received);
    let kinds: Vec<&'static str> = decisions.iter().map(|(k, _, _)| *k).collect();
    assert_eq!(
        kinds,
        vec!["continue", "continue", "budget_limited"],
        "two continues then cap-stop",
    );
    assert_eq!(
        *hook_invocations.lock(),
        2,
        "hook fires exactly twice (cap=2), then runtime stops without re-invoking",
    );
    let (_, last_count, last_reason) = decisions.last().unwrap();
    assert_eq!(*last_count, 2, "budget_limited records the final count");
    assert!(
        last_reason
            .as_ref()
            .map(|s| s.contains("continuation cap reached"))
            .unwrap_or(false),
        "budget_limited reason mentions the cap, got {:?}",
        last_reason,
    );

    let audits = read_custom_entries(storage, "turn_end_decision").await;
    assert_eq!(audits.len(), 3);
    assert_eq!(audits[2]["decision"], "budget_limited");
}

// The in-harness `run_evaluator` API was removed in the multiagent rework: the goal
// evaluator now runs as the goal run's node via `multiagent::runner::run_agent`
// (tool-less judge, isolated in-memory session). Behavior is covered end-to-end in
// `crates/server/tests/goal_hook_e2e.rs` (node job registration, transcript capture,
// interrupt -> goal pause, parent-session isolation).

// ─────────────────────────────────────────────────────────────────────────────────────────
// Issue #110 sub-PR 1.5 — harness `control_plane_prompt` Custom audit emission
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Faux tool whose classifier returns `Prompt` so the agent loop routes through the
/// control-plane prompt channel, which the harness then audits.
struct PromptingTool {
    def: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl theway_core::AgentTool for PromptingTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }
    fn label(&self) -> &str {
        "prompter"
    }
    fn permission_classification(
        &self,
        _prepared_args: &serde_json::Value,
    ) -> theway_core::PermissionClassification {
        theway_core::PermissionClassification::Prompt {
            reason: "control-plane write under test".into(),
        }
    }
    async fn execute(
        &self,
        _id: &str,
        _params: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<theway_core::AgentToolUpdate>,
    ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
        Ok(theway_core::AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text(
                "did run".to_string(),
            )],
            details: serde_json::Value::Null,
            terminate: None,
        })
    }
}

/// Two-shot stream_fn: first message asks for `prompter` tool call, second is plain stop.
/// Used by the audit tests below to drive the classifier → prompt → audit pipeline.
fn faux_stream_fn_classifier_then_stop() -> theway_core::StreamFn {
    use std::sync::Mutex as SMutex;
    let counter = Arc::new(SMutex::new(0u32));
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        let counter = counter.clone();
        tokio::spawn(async move {
            let n = {
                let mut c = counter.lock().unwrap();
                let v = *c;
                *c = c.saturating_add(1);
                v
            };
            let msg = if n == 0 {
                let mut args = serde_json::Map::new();
                args.insert("k".into(), serde_json::json!("v"));
                theway_llm_provider::AssistantMessage {
                    role: theway_llm_provider::AssistantRole::Assistant,
                    content: vec![theway_llm_provider::ContentBlock::ToolCall(
                        theway_llm_provider::ToolCall {
                            id: "call_x".into(),
                            name: "prompter".into(),
                            arguments: args,
                            thought_signature: None,
                        },
                    )],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: theway_llm_provider::Usage::default(),
                    stop_reason: theway_llm_provider::StopReason::ToolUse,
                    error_message: None,
                    timestamp: 0,
                }
            } else {
                theway_llm_provider::AssistantMessage {
                    role: theway_llm_provider::AssistantRole::Assistant,
                    content: vec![theway_llm_provider::ContentBlock::text("ok done")],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: theway_llm_provider::Usage::default(),
                    stop_reason: theway_llm_provider::StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                }
            };
            let reason = match msg.stop_reason {
                theway_llm_provider::StopReason::ToolUse => {
                    theway_llm_provider::DoneReason::ToolUse
                }
                _ => theway_llm_provider::DoneReason::Stop,
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason,
                message: msg,
            });
        });
        stream
    })
}

#[tokio::test]
async fn control_plane_prompt_allow_writes_audit_entry() {
    use theway_core::{AgentTool, ControlPlanePromptDecision, OnControlPlanePromptHook};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];
    let prompt_hook: OnControlPlanePromptHook =
        Arc::new(|_req, _cancel| Box::pin(async move { ControlPlanePromptDecision::Allow }));
    opts.on_control_plane_prompt = Some(prompt_hook);

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(
        audits.len(),
        1,
        "expected exactly one control_plane_prompt audit, got {}",
        audits.len()
    );
    let audit = &audits[0];
    assert_eq!(audit["schema_version"], 1);
    assert_eq!(audit["tool_call_id"], "call_x");
    assert_eq!(audit["tool_name"], "prompter");
    assert_eq!(audit["decision"], "allow");
    let hash = audit["args_hash"].as_str().expect("args_hash string");
    assert_eq!(hash.len(), 64, "args_hash must be 64-hex SHA-256");
    assert!(
        audit["label"]
            .as_str()
            .map(|s| s.contains("prompter"))
            .unwrap_or(false),
        "label must mention the tool name, got {:?}",
        audit["label"]
    );
    assert!(audit["at"].is_string(), "at must be a string timestamp");
}

#[tokio::test]
async fn control_plane_prompt_deny_writes_audit_with_reason() {
    use theway_core::{AgentTool, ControlPlanePromptDecision, OnControlPlanePromptHook};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];
    let prompt_hook: OnControlPlanePromptHook = Arc::new(|_req, _cancel| {
        Box::pin(async move {
            ControlPlanePromptDecision::Deny {
                reason: Some("user pressed N".into()),
            }
        })
    });
    opts.on_control_plane_prompt = Some(prompt_hook);

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["decision"], "deny");
    assert_eq!(audits[0]["reason"], "user pressed N");
}

#[tokio::test]
async fn control_plane_prompt_no_hook_writes_audit_with_failclosed_deny() {
    use theway_core::AgentTool;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];
    // No on_control_plane_prompt hook — runtime fails closed AND still writes the
    // audit recording the rejection.

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["decision"], "deny");
    let reason = audits[0]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("no on_control_plane_prompt hook"),
        "no-hook deny reason should mention missing hook, got {reason:?}"
    );
}

#[tokio::test]
async fn control_plane_prompt_audit_caps_oversized_label() {
    use theway_core::{
        AgentTool, BeforeToolCallContext, BeforeToolCallHook, BeforeToolCallResult,
        ControlPlanePromptDecision, ControlPlanePromptRequest, OnControlPlanePromptHook,
    };

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn_classifier_then_stop());
    opts.tools = vec![Arc::new(PromptingTool {
        def: theway_llm_provider::Tool {
            name: "prompter".into(),
            description: "".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    }) as Arc<dyn AgentTool>];

    // Hook supplies an oversized label. Runtime keeps authoritative binding fields
    // (covered by sub-PR 1 regression test) AND the harness audit caps the label
    // to ≤ 200 chars on a char boundary, ending with the truncation marker.
    let oversized = "x".repeat(1000);
    let oversized_clone = oversized.clone();
    let before_hook: BeforeToolCallHook = Arc::new(
        move |_ctx: BeforeToolCallContext, _cancel: tokio_util::sync::CancellationToken| {
            let label = oversized_clone.clone();
            Box::pin(async move {
                BeforeToolCallResult {
                    block: false,
                    reason: None,
                    prompt: Some(ControlPlanePromptRequest {
                        tool_call_id: "spoofed".into(),
                        tool_name: "spoofed".into(),
                        args_hash: "deadbeef".into(),
                        label,
                        payload: serde_json::json!({}),
                        reason: "spoofed reason".into(),
                    }),
                }
            })
        },
    );
    opts.before_tool_call = Some(before_hook);
    let prompt_hook: OnControlPlanePromptHook =
        Arc::new(|_req, _cancel| Box::pin(async move { ControlPlanePromptDecision::Allow }));
    opts.on_control_plane_prompt = Some(prompt_hook);

    let harness = AgentHarness::new(opts);
    harness.prompt("run").await.unwrap();

    let audits = read_custom_entries(storage, "control_plane_prompt").await;
    assert_eq!(audits.len(), 1);
    let label = audits[0]["label"].as_str().expect("label string");
    let label_chars = label.chars().count();
    assert!(
        label_chars <= 200,
        "audit label must be capped at 200 chars, got {label_chars}",
    );
    assert!(
        label.ends_with('…'),
        "capped audit label should end with truncation marker, got tail of {label:?}",
    );
}
