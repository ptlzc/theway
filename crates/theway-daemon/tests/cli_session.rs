//! CLI / session integration test (Phase 5 of the harness refactor plan).
//!
//! Exercises the full happy path without going through `main.rs` or the TUI: THEWAY_DIR-scoped
//! sessions dir, harness assembly mirroring the binary, faux StreamFn for deterministic
//! responses, then drop everything and reopen via `SqliteSessionRepo` to verify the active
//! branch survived persistence.

use std::sync::Arc;

use tempfile::tempdir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, StreamFn,
    ThinkingLevel,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use theway_storage::sqlite_repo::SqliteSessionRepo;

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

/// StreamFn that always emits a Done with the supplied text. Equivalent to the harness_e2e
/// helper but inlined so this test stays self-contained.
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

/// Create → prompt twice → drop → reopen → build_context() returns both user + both assistant
/// messages on the active branch. Uses a real jsonl repo on a `tempdir()` so the test goes
/// through actual file IO.
#[tokio::test]
async fn create_persist_reopen_resume_round_trips() {
    let dir = tempdir().unwrap();
    let session_id;

    {
        let repo = SqliteSessionRepo::new(dir.path());
        let store = repo
            .create("/some/cwd")
            .await
            .expect("create jsonl session");
        session_id = theway_contract::session::SessionReader::get_metadata_json(&store)
            .await
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let session = Session::from_store(Arc::new(store));

        let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
        opts.thinking_level = ThinkingLevel::Off;
        opts.stream_fn = Some(faux_stream_fn("ack"));
        let harness = AgentHarness::new(opts);
        harness.prompt("first").await.unwrap();
        harness.prompt("second").await.unwrap();
    }

    // Drop everything above, then reopen by id and verify the persisted branch.
    let repo = SqliteSessionRepo::new(dir.path());
    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 1, "expected exactly one session file");

    let store = repo.open(&files[0]).await.unwrap();
    let reopened_id = theway_contract::session::SessionReader::get_metadata_json(&store)
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(
        reopened_id, session_id,
        "metadata id must survive close/reopen"
    );

    let reopened = Session::from_store(Arc::new(store));
    let ctx = reopened.build_context().await.unwrap();
    // 2 user prompts + 2 assistant replies on the active branch.
    assert_eq!(
        ctx.messages.len(),
        4,
        "expected 4 messages; got: {:#?}",
        ctx.messages
    );

    let texts: Vec<String> = ctx
        .messages
        .iter()
        .filter_map(|m| match m {
            theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(u)) => {
                match &u.content {
                    theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                    _ => None,
                }
            }
            theway_core::AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => {
                a.content.iter().find_map(|b| match b {
                    theway_llm_provider::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["first", "ack", "second", "ack"]);
}

/// `--resume`'s hand-rolled hydration moved into the harness in Phase 4. This test exercises
/// the harness API directly: seed a thinking-level change, a model change, and a user message
/// into a memory session, then build a *fresh* harness with cold defaults and verify
/// `rehydrate_from_session` mirrors all three into agent state.
#[tokio::test]
async fn rehydrate_after_reopen_mirrors_state_into_agent() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    session.append_thinking_level_change("high").await.unwrap();
    session.append_model_change("faux", "faux").await.unwrap();
    session
        .append_message(theway_core::AgentMessage::Llm(
            theway_llm_provider::Message::User(theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("prior-prompt".into()),
                timestamp: 0,
            }),
        ))
        .await
        .unwrap();

    // Cold-start harness — thinking off, the seeded model not present in any catalog.
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.thinking_level = ThinkingLevel::Off;
    opts.stream_fn = Some(faux_stream_fn("unused"));
    let harness = AgentHarness::new(opts);

    let ctx = harness.rehydrate_from_session().await.unwrap();
    assert_eq!(ctx.thinking_level, "high");
    assert!(ctx.model.is_some());

    let state = harness.agent().state();
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.thinking_level, Some(ThinkingLevel::High));
    // The faux model isn't in the embedded catalog → keep the cold-start model. The point is
    // that rehydrate didn't blow it away or panic.
    assert!(state.model.is_some());
}
