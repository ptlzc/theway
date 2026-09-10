//! Turn finalisation: `finalize_partial_turn` content filtering and
//! `finish_message` role-preserving transform.

use super::*;

#[tokio::test]
async fn finalize_partial_turn_keeps_only_messages_with_content() {
    // Arrange: assistant with no content must not be appended.
    let agent = agent();
    let empty = assistant_message(Vec::new());
    agent.state().streaming_message = Some(empty);
    let cancel = tokio_util::sync::CancellationToken::new();

    // Act
    finalize_partial_turn(&agent.inner.clone(), &cancel).await;

    // Assert
    assert!(agent.state().messages.is_empty());
    assert!(agent.state().streaming_message.is_none());

    // Arrange: assistant with text must be appended.
    let with_text = assistant_message(vec![ContentBlock::text("partial")]);
    agent.state().streaming_message = Some(with_text);

    // Act
    finalize_partial_turn(&agent.inner.clone(), &cancel).await;

    // Assert
    assert_eq!(agent.state().messages.len(), 1);
    assert!(agent.state().streaming_message.is_none());

    // Arrange: thinking-only partials carry no conversational content and
    // must not be appended (they serialize to an invalid empty assistant
    // message on OpenAI-style wire protocols — closes the Esc-interrupt
    // "Invalid assistant message" failure).
    let thinking_only = assistant_message(vec![ContentBlock::Thinking(
        theway_llm_provider::ThinkingContent {
            thinking: "reasoning without any answer yet".into(),
            ..Default::default()
        },
    )]);
    agent.state().streaming_message = Some(thinking_only);

    // Act
    finalize_partial_turn(&agent.inner.clone(), &cancel).await;

    // Assert
    assert_eq!(agent.state().messages.len(), 1);
    assert!(agent.state().streaming_message.is_none());

    // Arrange: thinking + text is conversational and must be appended.
    let with_thinking_and_text = assistant_message(vec![
        ContentBlock::Thinking(theway_llm_provider::ThinkingContent {
            thinking: "reasoning".into(),
            ..Default::default()
        }),
        ContentBlock::text("partial"),
    ]);
    agent.state().streaming_message = Some(with_thinking_and_text);

    // Act
    finalize_partial_turn(&agent.inner.clone(), &cancel).await;

    // Assert
    assert_eq!(agent.state().messages.len(), 2);
    assert!(agent.state().streaming_message.is_none());
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Coverage-gap additions for run_loop driver
// ──────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn finish_message_keeps_original_when_transform_changes_role() {
    let mut agent = agent();
    let original = user_message("keep me");
    let replacement = assistant_message(vec![ContentBlock::text("changed role")]);
    Arc::get_mut(&mut agent.inner)
        .unwrap()
        .options
        .transform_message = Some(Arc::new(move |_message, _cancel| {
        let replacement = replacement.clone();
        Box::pin(async move { replacement })
    }));
    let cancel = tokio_util::sync::CancellationToken::new();

    let finalized = finish_message(&agent.inner, original.clone(), &cancel).await;

    assert!(matches!(
        finalized,
        AgentMessage::Llm(PiMessage::User(_))
    ));
    let messages = agent.state().messages.clone();
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0],
        AgentMessage::Llm(PiMessage::User(_))
    ));
}
