//! Session-backed harness state: model and thinking persistence, `move_to`
//! rehydration, and unknown-model replay.

use super::*;

#[tokio::test]
async fn set_model_and_thinking_level_persist_entries() {
    let h = harness();

    let id = h.set_model(faux_model()).await.unwrap();
    assert_eq!(h.session().get_entry(&id).await.unwrap().unwrap().type_str(), "model_change");

    let id = h
        .set_thinking_level(ThinkingLevel::High)
        .await
        .unwrap();
    assert_eq!(
        h.session()
            .get_entry(&id)
            .await
            .unwrap()
            .unwrap()
            .type_str(),
        "thinking_level_change"
    );

    assert_eq!(h.agent().state().model.as_ref().unwrap().id, "faux");
    assert_eq!(
        h.agent().state().thinking_level,
        Some(ThinkingLevel::High)
    );
}

#[tokio::test]
async fn move_to_rehydrates_state_and_emits_branch_event() {
    let h = harness();
    let first = h.session().append_message(user_message("one")).await.unwrap();
    h.session().append_message(user_message("two")).await.unwrap();
    h.agent().state().messages = vec![user_message("two")];

    let mut rx = h.subscribe_session_broadcast();
    let summary_id = h
        .move_to(
            Some(&first),
            Some(BranchSummaryInput {
                summary: "back to one".into(),
                details: None,
                from_hook: false,
            }),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(h.session().leaf_id().await.unwrap().unwrap(), summary_id);
    // Rehydrated branch: the kept message plus the branch-summary marker.
    assert_eq!(h.agent().state().messages.len(), 2);

    let mut saw_branch = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, SessionEvent::Branch { .. }) {
            saw_branch = true;
        }
    }
    assert!(saw_branch, "move_to must emit a Branch event");
}

#[tokio::test]
async fn rehydrate_from_session_restores_messages_and_thinking() {
    let h = harness();
    h.session().append_message(user_message("from session")).await.unwrap();
    h.session()
        .append_thinking_level_change("high")
        .await
        .unwrap();
    h.agent().state().messages = Vec::new();
    h.agent().state().thinking_level = Some(ThinkingLevel::Off);

    let ctx = h.rehydrate_from_session().await.unwrap();

    assert_eq!(ctx.messages.len(), 1);
    assert_eq!(h.agent().state().messages.len(), 1);
    assert_eq!(h.agent().state().thinking_level, Some(ThinkingLevel::High));
}

#[tokio::test]
async fn rehydrate_from_session_ignores_unknown_model_and_invalid_thinking() {
    let h = harness();
    h.session()
        .append_model_change("unknown-provider", "unknown-model")
        .await
        .unwrap();
    h.session()
        .append_thinking_level_change("not-a-level")
        .await
        .unwrap();
    h.agent.state().model = None;
    h.agent.state().thinking_level = None;

    let ctx = h.rehydrate_from_session().await.unwrap();

    assert_eq!(ctx.model.as_ref().unwrap().model_id, "unknown-model");
    assert!(h.agent.state().model.is_none());
    assert_eq!(h.agent.state().thinking_level, None);
}
