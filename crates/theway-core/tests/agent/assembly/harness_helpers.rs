//! Harness free-function helpers: system prompt assembly, control-plane audit
//! labels, user-text extraction, persisted-run errors, and budget caps.

use super::*;

#[test]
fn build_system_prompt_combines_base_and_skills() {
    assert_eq!(build_system_prompt("", &[]), "");
    assert_eq!(build_system_prompt("base", &[]), "base");
    assert!(build_system_prompt("", &[skill("a", "body")]).contains("<skill"));
    let both = build_system_prompt("base", &[skill("a", "body")]);
    assert!(both.starts_with("base\n\n"));
    assert!(both.contains("<skills>"));
    assert!(both.contains("test skill"));
}

#[test]
fn turn_end_action_audit_str_maps_only_non_noop() {
    assert_eq!(TurnEndAction::Noop.as_audit_str(), None);
    assert_eq!(TurnEndAction::Stop.as_audit_str(), Some("stop"));
    assert_eq!(
        TurnEndAction::Pause {
            reason: "x".into()
        }
        .as_audit_str(),
        Some("pause")
    );
    assert_eq!(
        TurnEndAction::Continue {
            prompt: "x".into()
        }
        .as_audit_str(),
        Some("continue")
    );
}

#[test]
fn turn_end_decision_from_action_sets_none_payload() {
    let decision = TurnEndDecision::from(TurnEndAction::Stop);
    assert!(matches!(decision.action, TurnEndAction::Stop));
    assert!(decision.payload.is_none());
}

#[test]
fn preview_for_banner_truncates_with_ellipsis() {
    assert_eq!(preview_for_banner("short", 10), "short");
    assert_eq!(preview_for_banner("123456", 5), "12345…");
}

#[test]
fn cap_control_plane_audit_label_caps_at_200_chars() {
    let exact = "x".repeat(200);
    assert_eq!(cap_control_plane_audit_label(&exact), exact);
    let over = "y".repeat(201);
    let capped = cap_control_plane_audit_label(&over);
    assert_eq!(capped.chars().count(), 200);
    assert!(capped.ends_with('…'));
}

#[test]
fn extract_user_message_text_handles_text_blocks_and_empty() {
    let text = UserMessage {
        role: UserRole::User,
        content: UserContent::Text("hello".into()),
        timestamp: 0,
    };
    assert_eq!(extract_user_message_text(&text).unwrap(), "hello");

    let empty = UserMessage {
        role: UserRole::User,
        content: UserContent::Text(String::new()),
        timestamp: 0,
    };
    assert_eq!(extract_user_message_text(&empty), None);

    let blocks = UserMessage {
        role: UserRole::User,
        content: UserContent::Blocks(vec![
            UserContentBlock::text("a"),
            UserContentBlock::Image(ImageContent {
                data: "base64".into(),
                mime_type: "image/png".into(),
            }),
            UserContentBlock::text("b"),
        ]),
        timestamp: 0,
    };
    assert_eq!(extract_user_message_text(&blocks).unwrap(), "a\nb");
}

#[test]
fn extract_user_prompt_text_returns_none_for_non_user() {
    assert_eq!(extract_user_prompt_text(&assistant_message("hi")), None);
    let custom = AgentMessage::Custom(CustomMessage {
        role: "note".into(),
        timestamp: 0,
        payload: serde_json::Value::Null,
    });
    assert_eq!(extract_user_prompt_text(&custom), None);
    assert_eq!(
        extract_user_prompt_text(&user_message("prompt")),
        Some("prompt".into())
    );
}

#[test]
fn finish_persisted_run_prefers_run_error_then_persist_error() {
    let persist_errors = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let run_err = Err(AgentRunError::Other("run failed".into()));
    let err = finish_persisted_run(run_err, persist_errors.clone()).unwrap_err();
    assert!(err.to_string().contains("run failed"));

    persist_errors.lock().push(SessionError {
        code: SessionErrorCode::StorageFailure,
        message: "disk".into(),
    });
    let err = finish_persisted_run(Ok(()), persist_errors.clone()).unwrap_err();
    assert!(err.to_string().contains("session append message: disk"));

    persist_errors.lock().clear();
    assert!(finish_persisted_run(Ok(()), persist_errors).is_ok());
}

#[test]
fn check_budget_cap_uses_configured_cap() {
    let mut h = harness();
    h.budget_cap_usd = None;
    assert!(h.check_budget_cap().is_ok());

    h.budget_cap_usd = Some(0.0);
    let err = h.check_budget_cap().unwrap_err();
    assert!(err.to_string().contains("budget cap reached"));
}

#[test]
fn last_user_text_from_state_finds_most_recent_user_text() {
    let h = harness();
    h.agent.state().messages = vec![
        user_message("first"),
        assistant_message("reply"),
        user_message("second"),
    ];
    assert_eq!(h.last_user_text_from_state().unwrap(), "second");
}

#[test]
fn check_budget_cap_allows_when_cost_below_cap() {
    let mut h = harness();
    h.budget_cap_usd = Some(1.0);
    h.cost.reset();
    assert!(h.check_budget_cap().is_ok());
}

#[test]
fn harness_construction_falls_back_to_dot_cwd_when_blank() {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.runtime_extension_cwd = "   ".into();

    let _h = AgentHarness::new(opts);
}
