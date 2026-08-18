//! Mirrored tests for `stream_auth` — split out of src (see docs/rust-test-files.md).
//!
//! Bridged from a `stream_auth_tests` wrapper because the inline `mod tests`
//! already occupies the top-level bridge slot.

use super::super::*;

fn model(provider: &str) -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from(provider),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

#[test]
fn auth_wrapper_ignores_whitespace_only_stored_key() {
    let opts = apply_auth_to_simple_options(&model("faux"), None, |_| Some("   ".into()));

    assert_eq!(opts.base.api_key, None);
}

#[test]
fn auth_wrapper_replaces_whitespace_only_explicit_key() {
    let mut existing = theway_llm_provider::SimpleStreamOptions::default();
    existing.base.api_key = Some("   ".into());

    let opts = apply_auth_to_simple_options(&model("faux"), Some(&existing), |_| {
        Some("stored-faux-key".into())
    });

    assert_eq!(opts.base.api_key.as_deref(), Some("stored-faux-key"));
}

#[test]
fn user_message_builds_text_user_message() {
    let before = chrono::Utc::now().timestamp_millis();

    let message = user_message("hello");

    let AgentMessage::Llm(PiMessage::User(user)) = message else {
        panic!("expected an LLM user message, got {message:?}");
    };
    assert_eq!(user.role, theway_llm_provider::UserRole::User);
    match &user.content {
        theway_llm_provider::UserContent::Text(text) => assert_eq!(text, "hello"),
        other => panic!("expected text content, got {other:?}"),
    }
    assert!(
        user.timestamp >= before && user.timestamp <= chrono::Utc::now().timestamp_millis(),
        "timestamp should be current: {}",
        user.timestamp
    );
}

#[test]
fn stream_fn_with_auth_store_returns_callable_stream_fn() {
    let stream_fn = stream_fn_with_auth_store();
    let context = theway_llm_provider::Context::default();
    let options = theway_llm_provider::SimpleStreamOptions::default();

    // Act: invoking the returned function executes the auth-merge closure and
    // dispatches to `stream_simple`. The faux provider answers synchronously
    // without any network I/O.
    let _stream = stream_fn(&model("faux"), &context, Some(&options));

    // Also exercise the `None` options path.
    let _stream_none = stream_fn(&model("faux"), &context, None);
}
