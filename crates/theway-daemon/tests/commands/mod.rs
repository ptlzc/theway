//! Tests for `commands` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use crate::test_env::{EnvGuard, ENV_LOCK};
use theway_core::SkillSource;

fn custom_test_model(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        name: "Secret Free Model".into(),
        api: theway_llm_provider::Api::from("openai-responses"),
        provider: Provider::from(provider),
        base_url: "https://secret-base.example/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![theway_llm_provider::InputModality::Text],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 100_000,
        max_tokens: 4096,
        headers: Some(std::collections::HashMap::from([(
            "Authorization".into(),
            "Bearer sk-secret-should-not-leak".into(),
        )])),
        compat: None,
    }
}

#[test]
fn parse_splits_on_whitespace() {
    let (name, args) = parse("/model anthropic:claude").unwrap();
    assert_eq!(name, "model");
    assert_eq!(args, vec!["anthropic:claude".to_string()]);
}

#[test]
fn parse_keeps_quoted_args_together() {
    let (name, args) = parse("/say \"hello world\" again").unwrap();
    assert_eq!(name, "say");
    assert_eq!(args, vec!["hello world".to_string(), "again".to_string()]);
}

#[test]
fn parse_returns_none_for_non_slash() {
    assert!(parse("hello world").is_none());
    assert!(parse("/").is_none());
}

#[test]
fn model_spec_accepts_colon_slash_and_two_args() {
    assert_eq!(
        parse_model_spec("deepseek:deepseek-v4-pro"),
        Some(("deepseek", "deepseek-v4-pro"))
    );
    assert_eq!(
        parse_model_spec("deepseek/deepseek-v4-pro"),
        Some(("deepseek", "deepseek-v4-pro"))
    );
    assert_eq!(
        parse_model_spec("deepseek deepseek-v4-pro"),
        Some(("deepseek", "deepseek-v4-pro"))
    );
    assert_eq!(parse_model_spec("deepseek"), None);
}

#[test]
fn model_credential_hint_uses_only_selected_provider_credentials() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let _deepseek = EnvGuard::remove("DEEPSEEK_API_KEY");
    let _openai = EnvGuard::set("OPENAI_API_KEY", "sk-openai-should-not-count");

    let hint = model_credential_hint("deepseek").expect("deepseek key is missing");
    assert!(hint.contains("DEEPSEEK_API_KEY"), "{hint}");
    assert!(hint.contains("/login deepseek"), "{hint}");
    assert!(!hint.contains("OPENAI_API_KEY"), "{hint}");
    assert!(!hint.contains("sk-openai-should-not-count"), "{hint}");
}

#[test]
fn model_credential_hint_accepts_env_or_auth_store_for_selected_provider() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let _deepseek = EnvGuard::set("DEEPSEEK_API_KEY", "sk-deepseek-present");
    assert!(model_credential_hint("deepseek").is_none());
    drop(_deepseek);

    let mut store = crate::auth::AuthStore::default();
    store.set(
        "deepseek",
        crate::auth::ProviderCredential::ApiKey {
            value: "stored-deepseek".into(),
        },
    );
    store.save().unwrap();
    assert!(model_credential_hint("deepseek").is_none());
}

#[test]
fn registry_lookup_by_name_and_alias() {
    let r = Registry::with_builtins();
    assert!(r.find("quit").is_some());
    assert!(r.find("q").is_some());
    assert!(r.find("exit").is_some());
    assert!(r.find("session").is_some());
    assert!(r.find("triggers").is_some());
    assert!(r.find("nope").is_none());
}

#[test]
fn registry_and_help_do_not_expose_removed_hub_surface() {
    let r = Registry::with_builtins();
    for removed in ["hub", "endpoint", "config"] {
        assert!(
            r.find(removed).is_none(),
            "/{removed} should not be registered"
        );
    }

    let help = help_text(&r, None);
    for removed in ["/hub", "/endpoint", "hub.inject", "pie.0xfefe.me"] {
        assert!(
            !help.contains(removed),
            "help should not expose removed hub surface `{removed}`:\n{help}"
        );
    }
}

#[test]
fn model_help_summary_lists_builtin_providers_without_secrets() {
    let text = cli_model_help_text();
    assert!(text.contains("Supported providers"), "{text}");
    assert!(text.contains("anthropic("), "{text}");
    assert!(text.contains("openai("), "{text}");
    assert!(text.contains("~/.theway/models.json"), "{text}");
    assert!(text.contains("<cwd>/.theway/models.json"), "{text}");
    assert!(!text.contains("API_KEY"), "{text}");
    assert!(!text.contains("auth.json"), "{text}");
}

#[test]
fn help_topic_renders_command_usage_and_aliases() {
    let registry = Registry::with_builtins();
    let model = help_text(&registry, Some("model"));
    assert!(
        model.contains("/model [provider:model-id|list [provider]]"),
        "{model}"
    );
    assert!(model.contains("show or switch the active model"), "{model}");
    assert!(model.contains("more: /help model"), "{model}");

    let quit = help_text(&registry, Some("/quit"));
    assert!(quit.contains("/quit"), "{quit}");
    assert!(quit.contains("aliases: /exit, /q"), "{quit}");

    let goal_start = help_text(&registry, Some("goal-start"));
    assert!(goal_start.contains("/goal-start <prompt>"), "{goal_start}");
    assert!(
        goal_start.contains("start working on the active session goal"),
        "{goal_start}"
    );
}

#[test]
fn help_unknown_topic_gives_recovery_hint() {
    let registry = Registry::with_builtins();
    let text = help_text(&registry, Some("mod"));
    assert!(text.contains("unknown help topic: mod"), "{text}");
    assert!(text.contains("Did you mean /model?"), "{text}");
}

#[test]
fn model_catalog_includes_custom_models_without_secret_fields() {
    let provider = Provider::from("help-test-provider");
    let id = "secret-free";
    theway_llm_provider::register_custom_model(custom_test_model(&provider.0, id));

    let text = model_catalog_text(Some(&provider.0)).unwrap();
    assert!(text.contains("help-test-provider"), "{text}");
    assert!(text.contains(id), "{text}");
    assert!(text.contains("Secret Free Model"), "{text}");
    assert!(!text.contains("secret-base"), "{text}");
    assert!(!text.contains("sk-secret"), "{text}");
    assert!(!text.contains("Authorization"), "{text}");

    theway_llm_provider::unregister_custom_model(&provider, id);
}

#[test]
fn unknown_model_error_lists_candidates() {
    let message = unknown_model_error("anthropic", "definitely-not-a-model");
    assert!(message.contains("unknown model in catalog"), "{message}");
    assert!(message.contains("Candidates:"), "{message}");
    assert!(message.contains("claude"), "{message}");
}

#[test]
fn unknown_provider_error_lists_provider_candidates() {
    let groups = model_groups();
    let message = unknown_provider_error("definitely-not-a-provider", &groups);
    assert!(message.contains("unknown provider"), "{message}");
    assert!(message.contains("anthropic("), "{message}");
    assert!(message.contains("openai("), "{message}");
}

#[test]
fn render_triggers_status_summarizes_runtime_hooks_and_running() {
    let snapshot = NotificationStatusSnapshot {
        hooks: vec![NotificationHookStatus {
            state: HookState::Disconnected {
                reason: "protocol_mismatch".into(),
            },
            last_event_at: None,
            last_ack_at: None,
            last_error: Some("bad frame".into()),
            queued_count: 2,
            dropped_count: 3,
            deduped_count: 4,
            subscription_labels: vec!["repo c4pt0r/theway".into()],
            requires_attention: Some("upgrade hub".into()),
        }],
        runtime: crate::trigger_engine::runtime::TriggerRuntimeSnapshot {
            dedup_entries: 5,
            active_traces: 6,
            accepted_total: 7,
            deduped_total: 8,
            cycle_suppressed_total: 9,
        },
        running: vec![RunningTriggerState {
            trace_id: "trace-1".into(),
            source_label: "mcp:github".into(),
            event_label: "pr_merged".into(),
            started_at: chrono::DateTime::parse_from_rfc3339("2026-05-22T19:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            prompt_preview: "summarize release".into(),
        }],
    };

    let status = render_triggers_status(&snapshot).join("\n");
    assert!(status.contains("accepted=7"));
    assert!(status.contains("recent_traces=6"));
    assert!(status.contains("1 total"));
    assert!(status.contains("1 require attention"));
    assert!(status.contains("running=1"));
    assert!(status.contains("push trigger sources: 1 configured source"));

    let sources = render_trigger_sources(&snapshot.hooks).join("\n");
    assert!(sources.contains("disconnected (protocol_mismatch)"));
    assert!(sources.contains("queued=2"));
    assert!(sources.contains("subscriptions: repo c4pt0r/theway"));
    assert!(sources.contains("attention: upgrade hub"));

    let running = render_running_triggers(&snapshot.running).join("\n");
    assert!(running.contains("trace-1"));
    assert!(running.contains("mcp:github / pr_merged"));
    assert!(running.contains("summarize release"));
}

#[test]
fn collect_trigger_audit_rows_uses_preview_safe_fields_only() {
    let entries = vec![
        SessionTreeEntry::Custom {
            id: "ignored".into(),
            parent_id: None,
            timestamp: "2026-05-22T19:00:00Z".into(),
            custom_type: "not_trigger".into(),
            data: Some(serde_json::json!({"trace_id": "ignored"})),
        },
        SessionTreeEntry::Custom {
            id: "t1".into(),
            parent_id: None,
            timestamp: "2026-05-22T19:01:00Z".into(),
            custom_type: "trigger".into(),
            data: Some(serde_json::json!({
                "trace_id": "trace-a",
                "state": "permission_denied",
                "source_label": "mcp:github",
                "event_label": "pr_merged",
                "payload_summary": "safe summary",
                "evaluator_decision": {
                    "outcome": "accept",
                    "permission": "deny",
                    "reason": "policy says no",
                    "raw_payload": "must-not-render"
                },
                "payload": {"secret": "must-not-render"}
            })),
        },
        SessionTreeEntry::Custom {
            id: "r1".into(),
            parent_id: None,
            timestamp: "2026-05-22T19:02:00Z".into(),
            custom_type: "trigger_result".into(),
            data: Some(serde_json::json!({
                "trace_id": "trace-a",
                "success": false,
                "reason": "aborted"
            })),
        },
        SessionTreeEntry::Custom {
            id: "p1".into(),
            parent_id: None,
            timestamp: "2026-05-22T19:03:00Z".into(),
            custom_type: "trigger_promotion".into(),
            data: Some(serde_json::json!({
                "trace_id": "trace-a",
                "state": "pending",
                "redaction_status": "clean"
            })),
        },
    ];

    let rows = collect_trigger_audit_rows(&entries, 10);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].custom_type, "trigger_promotion");
    assert_eq!(rows[0].state, "pending");
    assert_eq!(rows[1].state, "failed");
    assert_eq!(rows[2].source_label.as_deref(), Some("mcp:github"));
    let rendered = render_trigger_audit(&rows).join("\n");
    assert!(rendered.contains("trace-a"));
    assert!(rendered.contains("safe summary"));
    assert!(rendered.contains("decision: accept"));
    assert!(rendered.contains("permission: deny"));
    assert!(rendered.contains("reason: policy says no"));
    assert!(rendered.contains("redaction_status=clean"));
    assert!(!rendered.contains("must-not-render"));
    assert!(!rendered.contains("payload"));
}

#[test]
fn trigger_decision_details_explain_dedup_and_cycle_states() {
    let dedup = trigger_decision_details(&serde_json::json!({
        "evaluator_decision": {
            "outcome": "deduped",
            "replacement_policy": "latest_replaces",
            "previous_trace_id": "trace-old",
            "raw_payload": "must-not-render",
        }
    }))
    .join("\n");
    assert!(dedup.contains("decision: deduped"));
    assert!(dedup.contains("previous_trace_id: trace-old"));
    assert!(dedup.contains("replacement_policy: latest_replaces"));
    assert!(!dedup.contains("must-not-render"));

    let cycle = trigger_decision_details(&serde_json::json!({
        "evaluator_decision": {
            "outcome": "cycle_suppressed",
            "hop_count": 6,
        }
    }))
    .join("\n");
    assert!(cycle.contains("decision: cycle_suppressed"));
    assert!(cycle.contains("hop_count: 6"));
}

#[test]
fn attach_skill_prompt_wraps_prompt_without_skill_body() {
    let wrapped = attach_skill_prompt("review this change", Some("review-pr"));

    assert!(wrapped.contains("Skill tool"));
    assert!(wrapped.contains("review-pr"));
    assert!(wrapped.contains("review this change"));
    assert!(!wrapped.contains("SECRET SKILL BODY"));

    assert_eq!(attach_skill_prompt("plain", None), "plain");
}

#[test]
fn skill_source_label_maps_enum_variants() {
    // `/skills` now renders the structured `Skill.source` field (set by the loader per
    // discovery root) instead of inferring source from the file_path string. Lock the
    // label mapping the listing depends on.
    assert_eq!(SkillSource::Builtin.label(), "builtin");
    assert_eq!(SkillSource::User.label(), "user");
    assert_eq!(SkillSource::Project.label(), "project");
}

#[test]
fn skill_source_parse_error_is_fixed_and_bounded() {
    let err = parse_skill_source("user-secret-token").unwrap_err();
    assert!(err.contains("expected one of"), "{err}");
    assert!(!err.contains("user-secret-token"), "{err}");
}
