//! Model/session command suites: `/thinking`, `/session export`, `/template`,
//! `/compact`, `/undo`, `/name`, `/quit`, `/login`, `save_api_key`, `/share`,
//! plus the unknown-command error path.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
    SessionTreeEntry, ThinkingLevel,
};

use super::helpers::*;
use crate::auth;
use crate::commands;

#[tokio::test]
async fn dispatch_thinking_command_updates_state_and_session() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.thinking_level = ThinkingLevel::Off;
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/thinking high", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    assert_eq!(
        harness.agent().state().thinking_level,
        Some(ThinkingLevel::High)
    );
    let entries = session.entries().await.unwrap();
    let saw_change = entries.iter().any(|e| {
        matches!(
            e,
            SessionTreeEntry::ThinkingLevelChange { thinking_level, .. } if thinking_level == "high"
        )
    });
    assert!(
        saw_change,
        "thinking_level_change entry must be persisted: {entries:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_session_export_writes_archive_with_bounded_output() {
    let _output_guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("repo");
    tokio::fs::create_dir_all(&cwd).await.unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(temp.path().join("sessions"));
    let store = repo
        .create(cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    let metadata = theway_contract::session::SessionReader::get_metadata_json(&store)
        .await
        .unwrap();
    let session = Session::from_store(Arc::new(store));
    session
        .append_custom(
            "test_payload",
            Some(serde_json::json!({"secret_transcript_marker": "do-not-render"})),
        )
        .await
        .unwrap();
    let session_id = metadata["id"].as_str().unwrap().to_string();
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    let registry = commands::Registry::with_builtins();
    let capture = OutputCapture::install();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: &session_id,
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome =
        commands::dispatch("/session export backup.theway-session", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert!(cwd.join("backup.theway-session").exists());
    let output = capture.text();
    assert!(output.contains(".theway-session archives include transcript and tool history"));
    assert!(output.contains("exported session archive"));
    assert!(!output.contains("do-not-render"), "{output}");
    assert!(!output.contains("secret_transcript_marker"), "{output}");
}

#[tokio::test]
async fn dispatch_unknown_command_runs_it_as_agent_prompt() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    // Issue #37: a leading `/` is not necessarily a command — a path like
    // `/notarealcommand` is a plain user message, not an error.
    let outcome = commands::dispatch("/notarealcommand", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert_eq!(prompt, "/notarealcommand");
        }
        other => panic!("expected RunAgentPrompt outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_template_returns_repl_owned_agent_work() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    let outcome = commands::dispatch("/template release version=1.2.3", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunPromptTemplate { name, vars } => {
            assert_eq!(name, "release");
            assert_eq!(vars.get("version").and_then(|v| v.as_str()), Some("1.2.3"));
        }
        other => panic!("expected RunPromptTemplate outcome, got {other:?}"),
    }
    assert!(
        session.entries().await.unwrap().is_empty(),
        "/template dispatch should not run the agent directly; the TUI owns Ctrl-C abort handling"
    );
}

#[tokio::test]
async fn dispatch_compact_returns_repl_owned_agent_work() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    let outcome = commands::dispatch("/compact keep decisions", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunCompaction { custom } => {
            assert_eq!(custom.as_deref(), Some("keep decisions"));
        }
        other => panic!("expected RunCompaction outcome, got {other:?}"),
    }
    assert!(
        session.entries().await.unwrap().is_empty(),
        "/compact dispatch should not run compaction directly; the TUI owns Ctrl-C abort handling"
    );
}

#[tokio::test]
async fn dispatch_undo_removes_last_turn_from_active_branch() {
    use theway_core::StreamFn;
    use theway_llm_provider::{
        AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
        ContentBlock, DoneReason, StopReason, Usage,
    };

    fn faux_stream(text: &'static str) -> StreamFn {
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

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream("ack-1"));
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    harness.prompt("hi").await.unwrap();

    // Sanity: there are now 2 messages on the active branch (1 user, 1 assistant).
    let before = session.build_context().await.unwrap().messages.len();
    assert_eq!(before, 2);

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    let outcome = commands::dispatch("/undo", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let after = session.build_context().await.unwrap().messages.len();
    assert_eq!(
        after, 0,
        "after /undo, both user + assistant should be off the active branch"
    );
}

#[tokio::test]
async fn dispatch_name_sets_session_name() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    let outcome = commands::dispatch("/name my-thing", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    assert_eq!(
        session.session_name().await.unwrap().as_deref(),
        Some("my-thing")
    );
}

#[tokio::test]
async fn dispatch_quit_returns_quit_outcome() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    // quit/clear/help are TUI-local commands (daemon-kernel-layers); the
    // daemon no longer dispatches them. Issue #37: an unmatched `/…` is a
    // plain user message, so these forward as agent prompts here.
    for input in ["/quit", "/exit", "/q"] {
        let outcome = commands::dispatch(input, &registry, &ctx).await;
        assert!(
            matches!(outcome, commands::CommandOutcome::RunAgentPrompt { .. }),
            "{input} should not be a daemon command"
        );
    }
}

#[tokio::test]
async fn dispatch_login_prompts_for_secret_instead_of_accepting_inline_key() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/login ds4", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::LoginSecret {
            provider,
            storage_key,
            recovery_command,
        } => {
            assert_eq!(provider, "ds4");
            assert!(storage_key.is_none());
            assert!(recovery_command.is_none());
        }
        other => panic!("expected LoginSecret outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_login_rejects_inline_secret_material() {
    let secret = "sk-inline-secret-should-not-be-accepted";
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch(&format!("/login ds4 {secret}"), &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(message) => {
            assert!(message.contains("usage: /login <provider>"), "{message}");
            assert!(
                !message.contains(secret),
                "error must not repeat inline secret: {message}"
            );
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn save_api_key_persists_without_printing_secret_material() {
    let _auth_guard = auth::ENV_LOCK.lock().unwrap();
    let _guard = THEWAY_DIR_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", temp.path());
    let secret = "sk-sentinel-login-secret-should-not-leak";

    let path = commands::save_api_key("ds4", secret).expect("save api key");
    assert_eq!(path, temp.path().join("auth.json"));

    let stored = auth::AuthStore::load_from(&path).expect("load auth store");
    match stored.get("ds4").expect("stored ds4 credential") {
        auth::ProviderCredential::ApiKey { value } => assert_eq!(value, secret),
        other => panic!("unexpected credential kind: {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_share_default_uses_gh_private_default_without_secret_flag() {
    let _guard = GH_BIN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let argv_log = temp.path().join("argv.txt");
    let shim = write_fake_gh(
        temp.path(),
        &fake_gh_argv_and_url(&argv_log, "https://gist.github.com/example/private"),
    );
    let _gh_bin_guard = EnvGuard::set("THEWAY_GH_BIN", shim);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test-share-default",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/share", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    let argv = std::fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("gist create"), "argv: {argv}");
    assert!(
        !argv.contains("--secret"),
        "argv must not include removed gh flag: {argv}"
    );
    assert!(
        !argv.contains("--public"),
        "default share should remain private: {argv}"
    );
}

#[tokio::test]
async fn dispatch_share_public_passes_public_flag() {
    let _guard = GH_BIN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let argv_log = temp.path().join("argv.txt");
    let shim = write_fake_gh(
        temp.path(),
        &fake_gh_argv_and_url(&argv_log, "https://gist.github.com/example/public"),
    );
    let _gh_bin_guard = EnvGuard::set("THEWAY_GH_BIN", shim);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test-share-public",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/share --public", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));
    let argv = std::fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--public"), "argv: {argv}");
    assert!(
        !argv.contains("--secret"),
        "argv must not include removed gh flag: {argv}"
    );
}

#[tokio::test]
async fn dispatch_share_preserves_gh_stderr_on_failure() {
    let _guard = GH_BIN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let shim = write_fake_gh(temp.path(), &fake_gh_fail_stderr());
    let _gh_bin_guard = EnvGuard::set("THEWAY_GH_BIN", shim);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test-share-failure",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
        inherit_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let outcome = commands::dispatch("/share", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(message) => {
            assert!(message.contains("gh gist create exited 1"), "{message}");
            assert!(message.contains("unknown flag: --secret"), "{message}");
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}
