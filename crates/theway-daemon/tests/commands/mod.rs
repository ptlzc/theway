//! Tests for `commands` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::test_env::{EnvGuard, ENV_LOCK};
use theway_core::SkillSource;

use std::path::Path;
use std::sync::Arc;
use theway_core::{AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage};
use theway_transport::commands::CommandCtx;

use crate::commands::DaemonCtx;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::runtime_storage::local_runtime_storage;

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
    // quit/clear/help are TUI-local commands now (daemon-kernel-layers); the
    // daemon registry owns the runtime set (auth/session/triggers/…).
    let r = Registry::with_builtins();
    assert!(r.find("quit").is_none());
    assert!(r.find("login").is_some());
    assert!(r.find("logout").is_some());
    assert!(r.find("sessions").is_some());
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
    assert!(
        text.contains("Provider names are model/credential namespaces"),
        "{text}"
    );
    assert!(text.contains("openai-completions"), "{text}");
    assert!(text.contains("openai-responses"), "{text}");
    assert!(text.contains("anthropic-messages"), "{text}");
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

    let login = help_text(&registry, Some("/login"));
    assert!(login.contains("/login"), "{login}");

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

    let registry = crate::triggers::dynamic::DynamicTriggerRegistry::default();
    let status = render_triggers_status(&snapshot, &registry).join("\n");
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

#[test]
fn preview_text_caps_and_preserves_short_text() {
    assert_eq!(preview_text("abc", 10), "abc");
    let capped = preview_text("abcdef", 3);
    assert_eq!(capped, "abc…");
    assert_eq!(preview_text("a\nb", 10), "a b");
}

#[test]
fn resolve_skill_shortcut_covers_registry_and_ambiguity_cases() {
    let registry = Registry::with_builtins();
    let disabled = Skill {
        name: "foo".into(),
        description: String::new(),
        file_path: "/tmp/foo/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: true,
        source: SkillSource::User,
    };
    let enabled_foo = Skill {
        name: "foo".into(),
        description: String::new(),
        file_path: "/tmp/foo/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };
    let enabled_bar = Skill {
        name: "bar".into(),
        description: String::new(),
        file_path: "/tmp/bar/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };

    // Registry command takes precedence over a same-named skill.
    let skills = vec![Skill {
        name: "model".into(),
        description: String::new(),
        file_path: "/tmp/model/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }];
    assert!(resolve_skill_shortcut(&skills, &registry, "model").unwrap().is_none());

    assert!(resolve_skill_shortcut(&[], &registry, "nope").unwrap().is_none());

    assert!(resolve_skill_shortcut(&[disabled], &registry, "foo").is_err());

    let dup_foo_a = Skill {
        name: "foo".into(),
        description: String::new(),
        file_path: "/tmp/foo-a/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };
    let dup_foo_b = Skill {
        name: "foo".into(),
        description: String::new(),
        file_path: "/tmp/foo-b/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };
    assert!(resolve_skill_shortcut(&[dup_foo_a, dup_foo_b], &registry, "foo").is_err());

    let skills = vec![enabled_foo, enabled_bar];
    let resolved = resolve_skill_shortcut(&skills, &registry, "foo").unwrap().unwrap();
    assert_eq!(resolved.name, "foo");
}

#[test]
fn skill_shortcuts_filters_duplicate_names() {
    let registry = Registry::with_builtins();
    let skill_a = Skill {
        name: "dup".into(),
        description: "a".into(),
        file_path: "/tmp/a/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    };
    let skill_b = Skill {
        name: "dup".into(),
        description: "b".into(),
        file_path: "/tmp/b/SKILL.md".into(),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::Project,
    };
    let shortcuts = skill_shortcuts(&[skill_a, skill_b], &registry);
    assert!(shortcuts.is_empty(), "{shortcuts:?}");
}

#[test]
fn args_tail_of_handles_short_input() {
    assert_eq!(args_tail_of("/name", "name"), "");
    assert_eq!(args_tail_of("/name ", "name"), "");
    assert_eq!(args_tail_of("/name hi there", "name"), "hi there");
}

fn command_test_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: Provider::from("faux"),
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

fn command_test_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn command_test_harness(session: Session) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        command_test_model(),
        session,
    )))
}

fn command_test_executor(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

fn command_test_ctx<'a>(
    extra: &'a DaemonCtx,
    cwd: &'a Path,
) -> CommandCtx<'a, DaemonCtx> {
    CommandCtx {
        session_id: "test-session",
        log_path: None,
        tool_count: 0,
        cwd,
        extra,
    }
}

fn command_test_daemon_ctx(
    harness: &Arc<AgentHarness>,
    executor: Arc<TriggerExecutor>,
) -> DaemonCtx {
    DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor,
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
        inherit_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

#[tokio::test]
async fn model_command_without_args_opens_picker() {
    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::ModelCommand.run(&[], &ctx).await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::OpenModelPicker
    ));
}

#[tokio::test]
async fn model_command_list_known_and_unknown_provider() {
    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::ModelCommand
        .run(&["list".into(), "openai".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Handled
    ));

    let outcome = crate::commands::model::ModelCommand
        .run(&["list".into(), "definitely-not-a-provider".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Error(ref msg) if msg.contains("unknown provider")
    ));
}

#[tokio::test]
async fn model_command_unknown_model_and_bad_spec_are_errors() {
    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::ModelCommand
        .run(&["anthropic".into(), "definitely-not-a-model".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Error(ref msg) if msg.contains("unknown model")
    ));

    let outcome = crate::commands::model::ModelCommand
        .run(&["not-a-spec".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Error(ref msg) if msg.contains("expected provider:model-id")
    ));
}

#[tokio::test]
async fn model_command_switches_to_custom_model_without_credential_hint() {
    let provider_name = "command-test-provider";
    let provider = Provider::from(provider_name);
    let id = "command-test-model";
    theway_llm_provider::register_custom_model(custom_test_model(provider_name, id));

    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::ModelCommand
        .run(&[format!("{provider_name}:{id}")], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Handled
    ));

    theway_llm_provider::unregister_custom_model(&provider, id);
}

#[tokio::test]
async fn thinking_command_show_set_and_invalid_level() {
    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::ThinkingCommand.run(&[], &ctx).await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Handled
    ));

    let outcome = crate::commands::model::ThinkingCommand
        .run(&["high".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Handled
    ));

    let outcome = crate::commands::model::ThinkingCommand
        .run(&["bogus".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Error(ref msg) if msg.contains("invalid level")
    ));
}

#[tokio::test]
async fn cost_command_show_and_reset_are_handled() {
    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::CostCommand.run(&[], &ctx).await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Handled
    ));

    let outcome = crate::commands::model::CostCommand
        .run(&["reset".into()], &ctx)
        .await;
    assert!(matches!(
        outcome,
        theway_transport::commands::CommandOutcome::Handled
    ));
}

#[test]
fn model_catalog_text_handles_model_name_equal_to_id() {
    let provider_name = "command-test-catalog-provider-2";
    let provider = Provider::from(provider_name);
    let mut model = custom_test_model(provider_name, "same-name-model");
    model.name = model.id.clone();
    theway_llm_provider::register_custom_model(model);

    let text = crate::commands::model::model_catalog_text(Some(provider_name)).unwrap();
    assert!(text.contains("same-name-model"), "{text}");

    theway_llm_provider::unregister_custom_model(&provider, "same-name-model");
}

#[tokio::test]
async fn model_command_list_without_provider_is_handled() {
    let session = command_test_session();
    let harness = command_test_harness(session);
    let executor = command_test_executor(&harness);
    let extra = command_test_daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_test_ctx(&extra, tmp.path());

    let outcome = crate::commands::model::ModelCommand
        .run(&["list".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[test]
fn model_catalog_text_known_provider_and_empty_model_name() {
    let provider_name = "command-test-catalog-provider";
    let provider = Provider::from(provider_name);
    let mut model = custom_test_model(provider_name, "empty-name-model");
    model.name.clear();
    theway_llm_provider::register_custom_model(model);

    let text = crate::commands::model::model_catalog_text(Some(provider_name)).unwrap();
    assert!(text.contains("empty-name-model"), "{text}");

    theway_llm_provider::unregister_custom_model(&provider, "empty-name-model");
}

#[test]
fn unknown_model_error_for_unknown_provider_falls_back_to_provider_error() {
    let message = unknown_model_error("definitely-not-a-provider", "some-id");
    assert!(message.contains("unknown provider"), "{message}");
}

#[test]
fn unknown_model_error_lists_more_hint_for_large_catalog() {
    let message = crate::commands::model::unknown_model_error("openai", "definitely-not-a-model");
    assert!(message.contains("unknown model in catalog"), "{message}");
    assert!(message.contains("run /model list openai for all"), "{message}");
}
