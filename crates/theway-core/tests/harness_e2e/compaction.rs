//! Compaction tests: force_compact, auto-compaction bounds, trigger_custom handling.

use super::helpers::{faux_model, faux_stream_fn, user_message};
use super::*;

// ──────────────────────────────────────────────────────────────────────────────────────────
// Issue #19 regression tests — compaction `first_kept_entry_id` must be reachable in the
// session jsonl so `--resume` reconstructs the kept tail.
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Round-trip: drive the harness through a few turns + force_compact, then drop the harness,
/// rebuild the context from the session entries, and verify `build_session_context` reproduces
/// what was in in-memory state after compaction. The pre-fix bug was that
/// `first_kept_entry_id` written to the session referenced a synthetic id that no real entry
/// carried, so the rebuilt branch dropped the entire pre-compaction tail.
#[tokio::test]
async fn force_compact_writes_reachable_first_kept_entry_id_and_resume_preserves_tail() {
    let storage = std::sync::Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as std::sync::Arc<dyn SessionStorage>);

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

    // Rebuild the context from the session entries (same data a resume would replay).
    let entries = session.entries().await.unwrap();
    let rebuilt = build_session_context(&entries);

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
/// diagnostic `SessionEvent::Compaction` whose summary starts with `compaction skipped:` so
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

    // Collect SessionEvent::Compaction emissions.
    let events: Arc<PlMutex<Vec<SessionEvent>>> = Arc::new(PlMutex::new(Vec::new()));
    let events_clone = events.clone();
    let _unsub = harness.subscribe_harness(Arc::new(move |ev: SessionEvent| {
        events_clone.lock().push(ev);
    }) as SessionListener);

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
        SessionEvent::Compaction {
            summary,
            tokens_before,
            ..
        } => summary.starts_with("compaction skipped:") && *tokens_before == 0,
        _ => false,
    });
    assert!(
        saw_diagnostic,
        "expected a diagnostic SessionEvent::Compaction (summary starts with 'compaction skipped:') — events: {:?}",
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
