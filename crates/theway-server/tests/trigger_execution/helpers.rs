//! Shared fixtures for the trigger-execution suite: the faux model/stream builders, a
//! sample MCP trigger envelope, and the event-log polling helper used across domains.

use super::*;

pub(crate) fn faux_model() -> theway_llm_provider::Model {
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

pub(crate) fn faux_stream_fn(text: &'static str) -> StreamFn {
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

pub(crate) fn sample_trigger(
    idempotency_key: &str,
    trace_id: &str,
) -> theway::trigger_engine::types::Trigger {
    theway::trigger_engine::types::Trigger {
        source: theway::trigger_engine::types::TriggerSource::Mcp {
            server_name: "github".into(),
            method: "notifications/pr.merged".into(),
        },
        source_kind: theway::trigger_engine::types::SourceKind::Mcp,
        source_label: "MCP github".into(),
        event_label: "pr merged".into(),
        payload_visibility: theway::trigger_engine::types::PayloadVisibility::Local,
        payload_summary: Some("PR #42 merged".into()),
        payload: None,
        idempotency_key: idempotency_key.into(),
        replacement_policy: theway::trigger_engine::types::ReplacementPolicy::Drop,
        trace_id: trace_id.into(),
        authority: theway::trigger_engine::types::TriggerAuthority {
            principal_id: "mcp:github".into(),
            principal_label: "github".into(),
            credential_scope: theway::trigger_engine::types::CredentialScope::Project,
            allowed_source_actions: vec!["read".into()],
            expires_at: None,
        },
        received_at: chrono::Utc::now(),
    }
}

/// Helper: wait until a predicate over the captured event log returns Some(value) or the
/// deadline elapses. Polls every 20ms.
pub(crate) async fn wait_for_event<F, T>(
    events: &Arc<std::sync::Mutex<Vec<TriggerEvent>>>,
    timeout_secs: u64,
    mut pred: F,
) -> Option<T>
where
    F: FnMut(&[TriggerEvent]) -> Option<T>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Some(v) = pred(&events.lock().unwrap()) {
            return Some(v);
        }
        if std::time::Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
