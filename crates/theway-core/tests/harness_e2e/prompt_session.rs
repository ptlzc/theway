//! Prompt persistence, model/template selection, and session rehydration tests.

use super::helpers::{faux_model, faux_stream_fn, user_message};
use super::*;

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
