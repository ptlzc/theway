//! Tests for `trigger_engine::execution::promotion` — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, TriggerAuthority,
};
use theway_core::{AgentOptions, MemorySessionStorage, SessionTreeEntry};

fn trigger_with_source(source: TriggerSource) -> Trigger {
    Trigger {
        source,
        source_kind: SourceKind::Local,
        source_label: "local:test".into(),
        event_label: "test event".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: Some("payload summary".into()),
        payload: None,
        idempotency_key: "idem-1".into(),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: "trace-1".into(),
        authority: TriggerAuthority {
            principal_id: "principal-1".into(),
            principal_label: "Principal One".into(),
            credential_scope: CredentialScope::User,
            allowed_source_actions: vec!["read".into()],
            expires_at: None,
        },
        received_at: chrono::Utc::now(),
    }
}

fn mcp_trigger() -> Trigger {
    let mut t = trigger_with_source(TriggerSource::Mcp {
        server_name: "github".into(),
        method: "notifications/pr.merged".into(),
    });
    t.source_kind = SourceKind::Mcp;
    t.source_label = "MCP github".into();
    t
}

fn ctx_with_trace_id() -> std::collections::HashMap<String, String> {
    let mut ctx = std::collections::HashMap::new();
    ctx.insert("trace_id".into(), "trace-ctx".into());
    ctx.insert("trigger.payload".into(), "secret".into());
    ctx
}

#[test]
fn build_template_context_maps_mcp_local_and_agent_delegate_sources() {
    let trigger = mcp_trigger();
    let ctx = build_template_context("trace-ctx", &trigger, true, &Some("sum".into()), 2);
    assert_eq!(ctx.get("trace_id").unwrap(), "trace-ctx");
    assert_eq!(ctx.get("trigger.source.kind").unwrap(), "mcp");
    assert_eq!(
        ctx.get("trigger.source.server_name").unwrap(),
        "github"
    );
    assert_eq!(
        ctx.get("trigger.source.method").unwrap(),
        "notifications/pr.merged"
    );
    assert_eq!(ctx.get("trigger.source.subkind"), None);
    assert_eq!(ctx.get("trigger.source_label").unwrap(), "MCP github");
    assert_eq!(ctx.get("trigger.event_label").unwrap(), "test event");
    assert_eq!(ctx.get("trigger.payload_summary").unwrap(), "payload summary");
    assert!(!ctx.get("trigger.received_at").unwrap().is_empty());
    assert_eq!(ctx.get("trigger.idempotency_key").unwrap(), "idem-1");
    assert_eq!(
        ctx.get("trigger.authority.principal_id").unwrap(),
        "principal-1"
    );
    assert_eq!(
        ctx.get("trigger.authority.principal_label").unwrap(),
        "Principal One"
    );
    assert_eq!(
        ctx.get("trigger.authority.credential_scope").unwrap(),
        "User"
    );
    assert_eq!(ctx.get("result.summary").unwrap(), "sum");
    assert_eq!(ctx.get("result.status").unwrap(), "success");
    assert_eq!(ctx.get("result.message_count").unwrap(), "2");
    assert_eq!(ctx.get("result.cost_usd").unwrap(), "null");
    assert_eq!(ctx.get("result.branch_id").unwrap(), "null");

    let local = trigger_with_source(TriggerSource::Local {
        subkind: "cron".into(),
    });
    let ctx = build_template_context("trace-ctx", &local, false, &None, 0);
    assert_eq!(ctx.get("trigger.source.kind").unwrap(), "local");
    assert_eq!(ctx.get("trigger.source.server_name"), None);
    assert_eq!(ctx.get("trigger.source.method"), None);
    assert_eq!(ctx.get("trigger.source.subkind").unwrap(), "cron");
    assert_eq!(ctx.get("result.summary").unwrap(), "");
    assert_eq!(ctx.get("result.status").unwrap(), "failed");
    assert_eq!(ctx.get("result.message_count").unwrap(), "0");

    let delegated = trigger_with_source(TriggerSource::AgentDelegate {
        agent_id: "agent-1".into(),
        delegation_id: "deleg-1".into(),
    });
    let ctx = build_template_context("trace-ctx", &delegated, true, &None, 1);
    assert_eq!(ctx.get("trigger.source.kind").unwrap(), "agent_delegate");
    assert_eq!(ctx.get("trigger.source.server_name"), None);
    assert_eq!(ctx.get("trigger.source.method"), None);
    assert_eq!(ctx.get("trigger.source.subkind"), None);
}

#[test]
fn render_promotion_template_tolerates_whitespace_and_rejects_bad_references() {
    let ctx = ctx_with_trace_id();

    let rendered = render_promotion_template("hello {{ trace_id }}!", &ctx).unwrap();
    assert_eq!(rendered, "hello trace-ctx!");

    let rendered = render_promotion_template("{{  trace_id  }}", &ctx).unwrap();
    assert_eq!(rendered, "trace-ctx");

    assert!(matches!(
        render_promotion_template("{{ missing }}", &ctx),
        Err(TemplateRenderError::UnknownField(name)) if name == "missing"
    ));

    assert!(matches!(
        render_promotion_template("{{ trigger.payload }}", &ctx),
        Err(TemplateRenderError::ForbiddenField(name)) if name == "trigger.payload"
    ));
    assert!(matches!(
        render_promotion_template("{{ trigger.authority.allowed_source_actions }}", &ctx),
        Err(TemplateRenderError::ForbiddenField(name))
            if name == "trigger.authority.allowed_source_actions"
    ));
    assert!(matches!(
        render_promotion_template("{{ _meta.foo }}", &ctx),
        Err(TemplateRenderError::ForbiddenField(name)) if name == "_meta.foo"
    ));
    assert!(matches!(
        render_promotion_template("prefix {{ trace_id", &ctx),
        Err(TemplateRenderError::UnknownField(_))
    ));
}

#[test]
fn truncate_on_char_boundary_small_cap_returns_marker_only() {
    // Cap smaller than the marker: the function must fall back to marker-only
    // output rather than panicking on an underflowing budget.
    let (out, truncated) = truncate_on_char_boundary("x".repeat(32), TRUNCATION_MARKER.len() - 1);
    assert!(truncated);
    assert_eq!(out, TRUNCATION_MARKER);
    assert_eq!(out.len(), TRUNCATION_MARKER.len());
}

#[test]
fn truncate_on_char_boundary_multibyte_is_never_split() {
    let body = "é".repeat(100);
    let (out, truncated) = truncate_on_char_boundary(body, 50);
    assert!(truncated);
    assert!(out.len() <= 50, "out len {} must be <= cap 50", out.len());
    assert!(out.ends_with(TRUNCATION_MARKER));
    assert!(out.starts_with('é'));
}

#[test]
fn sha256_hex_matches_known_vectors() {
    assert_eq!(
        sha256_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(sha256_hex("abc").len(), 64);
}

#[test]
fn ensure_trigger_prefix_only_idempotent_for_current_trace_id() {
    let (body, injected) = ensure_trigger_prefix("hello".into(), "trace-1");
    assert_eq!(body, "[Trigger trace-1] hello");
    assert!(injected);

    let (body, injected) = ensure_trigger_prefix("[Trigger trace-1] hello".into(), "trace-1");
    assert_eq!(body, "[Trigger trace-1] hello");
    assert!(!injected);

    // A stale/different trace prefix is not trusted.
    let (body, injected) =
        ensure_trigger_prefix("[Trigger trace-evil] hello".into(), "trace-1");
    assert_eq!(body, "[Trigger trace-1] [Trigger trace-evil] hello");
    assert!(injected);
}

#[test]
fn compute_sub_agent_outcome_error_without_assistant_text() {
    let sub_agent = Agent::new(AgentOptions::default());
    let run_outcome = Err(AgentRunError::Other("boom".into()));

    let (success, summary, message_count) = compute_sub_agent_outcome(&sub_agent, &run_outcome);

    assert!(!success);
    assert_eq!(summary, None);
    assert_eq!(message_count, 0);
}

fn assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(theway_llm_provider::AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![theway_llm_provider::ContentBlock::text(text)],
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
    }))
}

#[test]
fn last_assistant_text_joins_text_blocks_and_returns_none_when_absent() {
    let state = AgentState::default();
    assert_eq!(last_assistant_text(&state), None);

    let mut state = AgentState::default();
    state.messages.push(assistant_message(""));
    assert_eq!(last_assistant_text(&state), None);

    let mut state = AgentState::default();
    let msg = AgentMessage::Llm(PiMessage::Assistant(theway_llm_provider::AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![
            theway_llm_provider::ContentBlock::text("hello"),
            theway_llm_provider::ContentBlock::text("world"),
        ],
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
    }));
    state.messages.push(msg);
    assert_eq!(last_assistant_text(&state).as_deref(), Some("hello\nworld"));
}

fn listener_vec(
    events: &Arc<std::sync::Mutex<Vec<TriggerEvent>>>,
) -> Arc<Mutex<Vec<TriggerListener>>> {
    let sink = events.clone();
    Arc::new(Mutex::new(vec![Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    })]))
}

#[tokio::test]
async fn apply_promotion_none_is_a_noop() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn theway_core::SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &PromoteAction::None,
        false,
        &serde_json::Value::Null,
    )
    .await;

    assert!(events.lock().unwrap().is_empty());
    let entries = session.entries().await.unwrap();
    assert!(
        !entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        )),
        "no promote -> no trigger_promotion audit"
    );
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "no promote -> no inserted message"
    );
}

#[tokio::test]
async fn apply_promotion_when_result_details_match_inserts_promoted_message() {
    use crate::trigger_engine::execution::types::PromotionCondition;

    let session = Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn theway_core::SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryWhenResultDetailsMatch {
        template_body: None,
        condition: PromotionCondition::AnyOf {
            json_pointer: "/matched_rule_ids".into(),
            any_of: vec!["dyn-keep".into()],
        },
    };
    let details = serde_json::json!({ "matched_rule_ids": ["dyn-keep", "dyn-other"] });

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sub-agent summary".into()),
        2,
        None,
        &promote,
        false,
        &details,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    let promoted = evs
        .iter()
        .find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                template_name,
                redaction_status,
                ..
            } if trace_id == "trace-1" => Some((
                inserted_entry_id.clone(),
                template_name.clone(),
                redaction_status.clone(),
            )),
            _ => None,
        })
        .expect("structured promotion must emit TriggerPromoted");
    assert_eq!(promoted.2, "clean");
    assert_eq!(promoted.1.as_deref(), Some("default"));

    let entries = session.entries().await.unwrap();
    let inserted = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &promoted.0 => Some(u.clone()),
            _ => None,
        })
        .expect("promoted message must be inserted");
    let theway_llm_provider::UserContent::Text(body) = &inserted.content else {
        panic!("promoted message must be text, got {:?}", inserted.content);
    };
    assert!(body.starts_with("[Trigger trace-1] "), "{body}");
    assert!(body.contains("sub-agent summary"), "{body}");
}

#[tokio::test]
async fn apply_promotion_render_error_emits_persistence_error_and_no_message() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn theway_core::SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryNow {
        template_body: Some("{{ trigger.authority.allowed_source_actions }}".into()),
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &promote,
        false,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, }
            if context == "trigger_promotion" && message.contains("forbidden template field")
    )));
    let entries = session.entries().await.unwrap();
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, SessionTreeEntry::Message { .. })),
        "failed render must not insert a message"
    );
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_promotion" => Some(d.clone()),
            _ => None,
        })
        .expect("render failure must still audit");
    assert_eq!(audit["state"], "failed");
    assert_eq!(audit["redaction_status"], "forbidden_field");
}
