//! Promotion prefix enforcement — no double-prefix, stale prefix, final length cap.

use super::delivery::promoting_action_hook;
use super::*;

#[tokio::test]
async fn promote_default_template_does_not_get_double_prefixed() {
    // Idempotency: the default template already starts with `[Trigger {{trace_id}}]`, so
    // the engine must NOT prepend a second prefix. Audit reflects prefix_injected = false.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
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

    let _ = executor
        .handle_trigger(sample_trigger("k-default-pfx", "trace-default-pfx"))
        .await;
    // Wait for completion.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
    assert_eq!(audit["prefix_injected"].as_bool(), Some(false));
    let user_msg_body = entries.iter().find_map(|e| match e {
        SessionTreeEntry::Message {
            message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
            ..
        } => match &u.content {
            theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    });
    let body = user_msg_body.expect("inserted user message");
    let prefix_occurrences = body.matches("[Trigger trace-default-pfx]").count();
    assert_eq!(
        prefix_occurrences, 1,
        "default template MUST NOT be double-prefixed; got body={body:?}"
    );
}

/// QA review on PR #65 a98c70b: `ensure_trigger_prefix` did `body.starts_with("[Trigger ")`
/// which would accept ANY `[Trigger ...]` prefix — including one a malicious template
/// embeds with a fake trace id. Fix: require the exact `[Trigger {trace_id}] ` form;
/// otherwise still inject the real prefix so the authoritative trace id wins.
#[tokio::test]
async fn promote_template_with_stale_trigger_prefix_still_gets_real_trace_id_prepended() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    let stream_fn = Some(faux_stream_fn("ok"));
    // Template carries a STALE / spoofed `[Trigger evil-trace-id]` prefix. The engine
    // must still prepend `[Trigger trace-real]` so the actual trace id is the first one
    // a reader sees; the stale one becomes embedded text.
    before_trigger_action = Some(promoting_action_hook(
        Some("[Trigger evil-trace-id] spoofed body for {{result.summary}}".into()),
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
        .handle_trigger(sample_trigger("k-stale-prefix", "trace-real"))
        .await;
    let inserted_entry_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                ..
            } if trace_id == "trace-real" => Some(inserted_entry_id.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted must fire");

    let entries = session.entries().await.unwrap();
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inserted user message");

    // Must start with the REAL trace id, not the stale one.
    assert!(
        body.starts_with("[Trigger trace-real] "),
        "real trace id MUST be prepended; got body={body:?}"
    );
    // The stale prefix appears as embedded text further in the body — proves the engine
    // didn't trust the user-supplied prefix.
    assert!(
        body.contains("[Trigger evil-trace-id]"),
        "stale prefix should remain as embedded text, body={body:?}"
    );
    // Audit reflects the real injection happened.
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_promotion" => data.clone(),
            _ => None,
        })
        .expect("audit");
    assert_eq!(audit["prefix_injected"].as_bool(), Some(true));
}

/// QA review on PR #65 a98c70b: previous truncation appended the marker AFTER cutting to
/// the cap, so the final body length = cap + marker.len() (~12 bytes over). Fix: cap is
/// the FINAL length including the marker.
#[tokio::test]
async fn promote_summary_truncation_final_length_includes_marker_under_cap() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook>;
    // Huge assistant text that triggers `last_assistant_text` truncation, which then feeds
    // a huge `{{result.summary}}` into the promotion template body. Both truncation sites
    // must respect the 4 KiB cap including marker.
    let stream_fn = {
        let huge_text: &'static str = Box::leak(("X".repeat(10 * 1024)).into_boxed_str());
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
        .handle_trigger(sample_trigger("k-cap-final", "trace-cap-final"))
        .await;
    let inserted_entry_id = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerPromoted {
                trace_id,
                inserted_entry_id,
                redaction_status,
                ..
            } if trace_id == "trace-cap-final" && redaction_status == "truncated" => {
                Some(inserted_entry_id.clone())
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerPromoted (truncated) must fire");

    let entries = session.entries().await.unwrap();
    let body = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Message {
                id,
                message: AgentMessage::Llm(theway_llm_provider::Message::User(u)),
                ..
            } if id == &inserted_entry_id => match &u.content {
                theway_llm_provider::UserContent::Text(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("inserted body");

    assert!(
        body.ends_with("…[truncated]"),
        "final body must end with truncation marker"
    );
    // The fix's contract: the FINAL body (including marker) is ≤ cap.
    assert!(
        body.len() <= 4096,
        "final inserted body (including marker) MUST respect 4 KiB cap; got {} bytes",
        body.len()
    );

    // Same invariant applies to trigger_result.summary that feeds into the template.
    let summary = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data
                .as_ref()
                .and_then(|d| d["summary"].as_str().map(String::from)),
            _ => None,
        })
        .expect("trigger_result.summary");
    assert!(
        summary.len() <= 4096,
        "trigger_result.summary (including marker) MUST respect 4 KiB cap; got {} bytes",
        summary.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// PromotionCondition — structured authorization gate for
// PromoteAction::PromoteSummaryWhenResultDetailsMatch. These tests pin the runtime
// contract directly (not through coding-agent's dynamic.rs path). Coverage:
//   - pointer-missing / value-not-array / empty-intersection → distinct skip reasons
//   - matching path → returns the intersection
//   - skip reasons stringify to stable audit identifiers
// ─────────────────────────────────────────────────────────────────────────────────────────
