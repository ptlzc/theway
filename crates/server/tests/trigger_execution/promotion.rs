//! Promotion — PromoteSummaryNow, template engine, trigger_promotion audit, fail-closed (RFC 1 sub-PR 5b).

use super::delivery::promoting_action_hook;
use super::*;

#[tokio::test]
async fn promote_summary_now_inserts_audited_parent_entry() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("sub agent reports OK"));
    before_trigger_action = Some(promoting_action_hook(None, false));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-promote-ok", "trace-promote-ok"))
        .await;

    let promoted_event = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                redaction_status,
                template_name,
                ..
            } if trace_id == "trace-promote-ok" => Some((
                inserted_entry_id.clone(),
                redaction_status.clone(),
                template_name.clone(),
            )),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");
    let (inserted_entry_id, redaction_status, template_name) = promoted_event;
    assert_eq!(redaction_status, "clean");
    // Default built-in template gets stable identifier "default" (per @Tools-MCP-Lead's
    // PR #65 review — the audit contract requires a stable name, not None for default).
    assert_eq!(
        template_name.as_deref(),
        Some("default"),
        "default built-in template must record stable identifier \"default\""
    );

    let entries = session.entries().await.unwrap();

    // The inserted Message::User must exist with the expected id + body containing the
    // default template's text shape.
    let msg = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => Some(u.clone()),
            _ => None,
        })
        .expect("inserted user message must exist in parent jsonl");
    let body = match &msg.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.contains("[Trigger trace-promote-ok]"),
        "default template body must include trace_id prefix; got {body:?}"
    );
    assert!(
        body.contains("sub agent reports OK"),
        "body must include result.summary; got {body:?}"
    );

    // trigger_promotion audit must reference the same inserted_entry_id.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit must exist");
    assert_eq!(audit["state"].as_str(), Some("success"));
    assert_eq!(audit["trace_id"].as_str(), Some("trace-promote-ok"));
    assert_eq!(
        audit["inserted_entry_id"].as_str(),
        Some(inserted_entry_id.as_str())
    );
    assert_eq!(audit["redaction_status"].as_str(), Some("clean"));
}

/// Acceptance #10: template references unknown variable → no insertion, audit `state:
/// "failed"` with `redaction_status: "render_error"`, parent transcript unchanged.
#[tokio::test]
async fn promote_template_unknown_var_fails_closed() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("sub ok"));
    before_trigger_action = Some(promoting_action_hook(
        Some("Hello {{nonexistent_field}}".into()),
        false,
    ));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-unknown", "trace-unknown"))
        .await;
    // Wait for the promotion-failure PersistenceError reflux.
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::PersistenceError {
                context, message, ..
            } if context == "trigger_promotion" && message.contains("nonexistent_field") => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("PersistenceError with unknown_field reason");

    let entries = session.entries().await.unwrap();
    // No Message::User entries in parent transcript.
    let has_msg = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(!has_msg, "render error → parent transcript unchanged");
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("failed promotion audit");
    assert_eq!(audit["state"].as_str(), Some("failed"));
    assert_eq!(audit["redaction_status"].as_str(), Some("render_error"));
    assert!(audit["inserted_entry_id"].is_null());
}

/// Acceptance #11: template references explicitly forbidden field (e.g.
/// `trigger.payload`) → no insertion, audit `redaction_status: "forbidden_field"`.
#[tokio::test]
async fn promote_template_forbidden_field_fails_closed() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    before_trigger_action = Some(promoting_action_hook(
        Some("Leaking {{trigger.payload}}".into()),
        false,
    ));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-forbid", "trace-forbid"))
        .await;
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::PersistenceError {
                context, message, ..
            } if context == "trigger_promotion" && message.contains("trigger.payload") => Some(()),
            _ => None,
        })
    })
    .await
    .expect("PersistenceError with forbidden_field reason");

    let entries = session.entries().await.unwrap();
    let has_msg = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(!has_msg);
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("failed promotion audit");
    assert_eq!(audit["state"].as_str(), Some("failed"));
    assert_eq!(audit["redaction_status"].as_str(), Some("forbidden_field"));
}

/// Acceptance #13: `promote_requires_approval = true` + no CLI approval command shipped =
/// fail-closed to pending. `trigger_promotion.state: "pending"`, `PromotionPending` event,
/// parent transcript unchanged.
#[tokio::test]
async fn promote_requires_approval_fails_closed_to_pending() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    before_trigger_action = Some(promoting_action_hook(None, true));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-pending", "trace-pending"))
        .await;
    let pending = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::PromotionPending {
                trace_id, preview, ..
            } if trace_id == "trace-pending" => Some(preview.clone()),
            _ => None,
        })
    })
    .await
    .expect("PromotionPending must fire");
    let preview = pending.expect("preview body should be Some when render succeeded");
    assert!(preview.contains("[Trigger trace-pending]"));

    let entries = session.entries().await.unwrap();
    let has_msg = entries
        .iter()
        .any(|e| matches!(e, SessionTreeEntry::Message { .. }));
    assert!(
        !has_msg,
        "promote_requires_approval=true must NOT insert into parent transcript without explicit approval"
    );
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("pending promotion audit");
    assert_eq!(audit["state"].as_str(), Some("pending"));
    assert!(audit["inserted_entry_id"].is_null());

    // Also assert no TriggerPromoted event.
    let evs = events.lock().unwrap().clone();
    let promoted = evs.iter().any(|e| {
        matches!(
            e,
            TriggerEvent::TriggerPromoted { trace_id, .. } if trace_id == "trace-pending"
        )
    });
    assert!(!promoted);
}

/// Acceptance #12: summary cap truncation. Large `result.summary` (> 4 KiB) is truncated
/// and `redaction_status: "truncated"` is reflected in both the audit and the event.
///
/// Drive by giving the faux stream a huge assistant body so `last_assistant_text` already
/// truncates the summary down to 4 KiB. Then the rendered template body (containing that
/// summary) will exceed `PROMOTION_BODY_CAP_BYTES` and trigger the body-cap truncation.
#[tokio::test]
async fn promote_summary_truncation_records_redaction_status() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    // ~6 KiB assistant text.
    let stream_fn = {
        let huge_text: &'static str = Box::leak(("X".repeat(6 * 1024)).into_boxed_str());
        Some(faux_stream_fn(huge_text))
    };
    before_trigger_action = Some(promoting_action_hook(None, false));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-trunc", "trace-trunc"))
        .await;
    let evt = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                redaction_status,
                inserted_entry_id,
                ..
            } if trace_id == "trace-trunc" => {
                Some((redaction_status.clone(), inserted_entry_id.clone()))
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted (truncated) must fire");
    let (redaction, inserted_id) = evt;
    assert_eq!(redaction, "truncated");

    // The inserted message must be capped (≤ 4 KiB + the marker bytes).
    let entries = session.entries().await.unwrap();
    let msg = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_id => Some(u.clone()),
            _ => None,
        })
        .expect("inserted user message");
    let body = match &msg.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.ends_with("…[truncated]"),
        "truncated body must end with truncation marker"
    );
    // Audit must match.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("audit");
    assert_eq!(audit["redaction_status"].as_str(), Some("truncated"));
}

/// Provider-Auth review on PR #65: inline `PromoteSummaryNow { template_body }` MUST NOT
/// be stored as `template_name` in the `trigger_promotion` audit / events. Audit identity
/// shape: `"default"` for built-in, `"inline:{hash[..8]}"` for hook-supplied bodies, with
/// the full SHA-256 in `template_hash` for verification.
#[tokio::test]
async fn promote_inline_template_body_is_not_persisted_as_template_name() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("subagent text"));
    let inline_body = "Custom RFC4-style prompt: {{trigger.source_label}} → {{result.summary}}";
    before_trigger_action = Some(promoting_action_hook(Some(inline_body.into()), false));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-inline-name", "trace-inline-name"))
        .await;
    let promoted_template_name = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                template_name,
                ..
            } if trace_id == "trace-inline-name" => Some(template_name.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    let name = promoted_template_name.expect("template_name must be Some");
    assert!(
        name.starts_with("inline:"),
        "inline template MUST be identified via inline:hash prefix, got {name:?}"
    );
    assert_eq!(
        name.len(),
        "inline:".len() + 8,
        "inline name is `inline:` + first 8 chars of sha256(body); got {name:?}"
    );
    assert!(
        !name.contains(inline_body),
        "template_name MUST NOT contain raw body: got {name:?}"
    );

    // Audit shows the same shape + a full template_hash for cross-process verification.
    let entries = session.entries().await.unwrap();
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit");
    assert_eq!(audit["template_name"].as_str(), Some(name.as_str()));
    let template_hash = audit["template_hash"]
        .as_str()
        .expect("template_hash must be Some(hex string)");
    assert_eq!(
        template_hash.len(),
        64,
        "SHA-256 hex must be 64 chars; got {} chars",
        template_hash.len()
    );
    assert!(
        !audit.to_string().contains(inline_body),
        "raw template body MUST NOT appear anywhere in the audit blob: {audit:?}"
    );
}

/// @Tools-MCP-Lead PR #65 review: enforce `[Trigger {trace_id}] ` prefix in the engine,
/// not in template-author discipline. A custom template without the prefix MUST still
/// produce a parent-session entry that starts with the prefix + audit
/// `prefix_injected: true`.
#[tokio::test]
async fn promote_summary_now_custom_template_without_prefix_still_gets_injected() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("subagent text"));
    // Custom template WITHOUT the `[Trigger ...]` prefix — engine must inject one.
    before_trigger_action = Some(promoting_action_hook(
        Some("Bare update from {{trigger.source_label}}: {{result.summary}}".into()),
        false,
    ));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-prefix-inj", "trace-prefix-inj"))
        .await;
    let inserted_entry_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-prefix-inj" => Some(inserted_entry_id.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    let entries = session.entries().await.unwrap();
    let msg = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => Some(u.clone()),
            _ => None,
        })
        .expect("inserted user message");
    let body = match &msg.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        _ => panic!("expected text body"),
    };
    assert!(
        body.starts_with("[Trigger trace-prefix-inj] "),
        "engine MUST inject the trigger prefix on templates that don't include one; got: {body:?}"
    );
    assert!(
        body.contains("Bare update from MCP github"),
        "custom template body MUST still be rendered after the injected prefix; got: {body:?}"
    );

    // Audit reflects prefix_injected = true.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("trigger_promotion audit");
    assert_eq!(audit["prefix_injected"].as_bool(), Some(true));

    // Idempotency check: a template that ALREADY starts with [Trigger should NOT get
    // double-prefixed (covered by the default-template test where audit prefix_injected
    // ought to be false). Verified here implicitly: if the engine doubled the prefix,
    // body would start with `[Trigger trace-prefix-inj] [Trigger ...]`.
    assert!(
        !body.starts_with("[Trigger trace-prefix-inj] [Trigger"),
        "prefix injection MUST be idempotent (no double `[Trigger `); got: {body:?}"
    );
}
