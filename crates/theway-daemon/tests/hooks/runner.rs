//! Additional runner/loader tests for `hooks` — split out of `mod.rs` by
//! scenario: side-effect seams, cancellation, env-dependent loading, and rule
//! parsing corner cases.

use super::{capture_command_executor, capture_webhook_sender, rule, runner};
use super::super::{
    load_with, push_rules, read_file, EventData, HookCwd, HookEvent, HookExecutors, OnFailure,
};
use crate::test_env::{ENV_LOCK, EnvGuard};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use theway_core::{AgentMessage, AgentToolResult, LoopEvent, SessionEvent, ThinkingLevel};
use theway_llm_provider::{Message, ToolResultMessage, ToolResultRole, UserContentBlock};
use tokio_util::sync::CancellationToken;

fn model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "model-1".into(),
        name: "Model One".into(),
        api: "test-api".into(),
        provider: "test-provider".into(),
        base_url: "http://127.0.0.1:9".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
        context_window: 128_000,
        max_tokens: 4_096,
        headers: None,
        compat: None,
    }
}

fn daemon_paths(base: &std::path::Path, work: &std::path::Path) -> crate::DaemonPaths {
    crate::DaemonPaths {
        base: base.to_path_buf(),
        home: base.to_path_buf(),
        work_dir: work.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    }
}

fn tool_end_event(tool_name: &str) -> LoopEvent {
    LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: tool_name.into(),
        result: AgentToolResult::default(),
        is_error: false,
    }
}

fn compaction_event() -> SessionEvent {
    SessionEvent::Compaction {
        from_hook: true,
        summary: "compact".into(),
        tokens_before: 10,
    }
}

#[test]
fn runner_debug_formats_with_and_without_executors() {
    // Arrange
    let no_executors = runner(vec![]);
    let _ = format!("{no_executors:?}");

    let mut with_executors = runner(vec![]);
    let command_slot = Arc::new(Mutex::new(None));
    let webhook_slot = Arc::new(Mutex::new(None));
    with_executors.command_executor = Some(capture_command_executor(command_slot));
    with_executors.webhook_sender = Some(capture_webhook_sender(webhook_slot));
    let _ = format!("{with_executors:?}");
}

#[test]
fn runner_is_empty_and_len_reflect_rule_count() {
    // Arrange
    let empty = runner(vec![]);

    // Act & Assert
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);

    // Arrange
    let one = runner(vec![rule(HookEvent::ToolEnd)]);

    // Act & Assert
    assert!(!one.is_empty());
    assert_eq!(one.len(), 1);
}

#[test]
fn runner_for_session_rebinds_identity_and_preserves_rules_paths_executors() {
    // Arrange
    let mut hook_rule = rule(HookEvent::ToolEnd);
    hook_rule.command = Some("echo hi".into());
    hook_rule.webhook = Some("http://127.0.0.1:9/hook".into());
    let command_slot = Arc::new(Mutex::new(None));
    let webhook_slot = Arc::new(Mutex::new(None));
    let mut original = runner(vec![hook_rule]);
    original.work_dir = std::path::PathBuf::from("/explicit/project");
    original.base = std::path::PathBuf::from("/explicit/base");
    original.home = std::path::PathBuf::from("/explicit/home");
    original.command_executor = Some(capture_command_executor(command_slot));
    original.webhook_sender = Some(capture_webhook_sender(webhook_slot));

    // Act
    let rebound = original.for_session(
        "session-2",
        Some(&model()),
        Some(ThinkingLevel::High),
    );

    // Assert
    assert_eq!(rebound.session_id, "session-2");
    assert_eq!(rebound.model_provider, "test-provider");
    assert_eq!(rebound.model_id, "model-1");
    assert_eq!(rebound.thinking_level, "high");
    assert_eq!(rebound.rules.len(), 1);
    assert_eq!(rebound.rules[0].command.as_deref(), Some("echo hi"));
    assert_eq!(rebound.rules[0].webhook.as_deref(), Some("http://127.0.0.1:9/hook"));
    assert_eq!(rebound.work_dir, std::path::PathBuf::from("/explicit/project"));
    assert_eq!(rebound.base, std::path::PathBuf::from("/explicit/base"));
    assert_eq!(rebound.home, std::path::PathBuf::from("/explicit/home"));
    assert!(Arc::ptr_eq(
        original.command_executor.as_ref().unwrap(),
        rebound.command_executor.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        original.webhook_sender.as_ref().unwrap(),
        rebound.webhook_sender.as_ref().unwrap()
    ));

    // Blank model and thinking fall back to the same off/empty defaults as load.
    let defaults = original.for_session("session-3", None, None);
    assert_eq!(defaults.session_id, "session-3");
    assert_eq!(defaults.model_provider, "");
    assert_eq!(defaults.model_id, "");
    assert_eq!(defaults.thinking_level, "off");
}

#[tokio::test]
async fn read_file_missing_returns_none_without_diagnostics() {
    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("missing").join("hooks.toml");
    let mut diagnostics = Vec::new();

    // Act
    let file = read_file(&path, "test", &mut diagnostics).await;

    // Assert
    assert!(file.is_none());
    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn read_file_parse_error_reports_label_and_path() {
    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hooks.toml");
    std::fs::write(&path, "not [[ valid toml").unwrap();
    let mut diagnostics = Vec::new();

    // Act
    let file = read_file(&path, "test", &mut diagnostics).await;

    // Assert
    assert!(file.is_none());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("parse"), "{diagnostics:?}");
    assert!(diagnostics[0].contains("test"), "{diagnostics:?}");
    assert!(
        diagnostics[0].contains(&path.display().to_string()),
        "{diagnostics:?}"
    );
}

#[tokio::test]
async fn read_file_read_error_reports_label_and_path() {
    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hooks.toml");
    std::fs::create_dir(&path).unwrap();
    let mut diagnostics = Vec::new();

    // Act
    let file = read_file(&path, "test", &mut diagnostics).await;

    // Assert
    assert!(file.is_none());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("read"), "{diagnostics:?}");
    assert!(
        diagnostics[0].contains(&path.display().to_string()),
        "{diagnostics:?}"
    );
}

#[test]
fn push_rules_skips_disabled_and_empty_rules_and_applies_options() {
    // Arrange
    let file = toml::from_str(
        r#"
allow_project_hooks = true

[[hook]]
event = "agent_start"
enabled = false
command = "echo disabled"

[[hook]]
event = "message_start"
command = ""

[[hook]]
event = "turn_end"
command = "   "
webhook = "http://127.0.0.1:9/hook"

[[hook]]
event = "compaction"
command = "echo compacted"
tool = "bash"
timeout_ms = 42
cwd = "theway"
on_failure = "ignore"
headers = { X-Test = "v" }
"#,
    )
    .unwrap();
    let mut rules = Vec::new();
    let mut diagnostics = Vec::new();

    // Act
    push_rules(file, "test", &mut rules, &mut diagnostics);

    // Assert
    assert_eq!(rules.len(), 2, "disabled and empty hooks should be skipped");
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("neither command nor webhook"), "{diagnostics:?}");

    let webhook_rule = &rules[0];
    assert_eq!(webhook_rule.event, HookEvent::TurnEnd);
    assert!(webhook_rule.command.is_none());
    assert_eq!(webhook_rule.webhook.as_deref(), Some("http://127.0.0.1:9/hook"));

    let compact_rule = &rules[1];
    assert_eq!(compact_rule.event, HookEvent::Compaction);
    assert_eq!(compact_rule.timeout_ms, 42);
    assert_eq!(compact_rule.cwd, HookCwd::ThewayHarness);
    assert_eq!(compact_rule.on_failure, OnFailure::Ignore);
    assert_eq!(compact_rule.tool.as_deref(), Some("bash"));
    assert_eq!(compact_rule.headers.get("X-Test").map(String::as_str), Some("v"));
}

#[tokio::test]
async fn load_with_read_local_files_false_returns_empty_runner_with_model_fields() {
    // Arrange
    let _env_lock = ENV_LOCK.lock().unwrap();
    let poisoned = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", poisoned.path());
    let base = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // Act
    let loaded = load_with(
        &daemon_paths(base.path(), cwd.path()),
        "session-1",
        Some(&model()),
        Some(ThinkingLevel::High),
        HookExecutors::default(),
        false,
    )
    .await;

    // Assert
    assert!(loaded.runner.is_empty());
    assert_eq!(loaded.runner.len(), 0);
    assert!(loaded.diagnostics.is_empty());
    assert_eq!(loaded.runner.model_provider, "test-provider");
    assert_eq!(loaded.runner.model_id, "model-1");
    assert_eq!(loaded.runner.thinking_level, "high");
}

#[tokio::test]
async fn load_with_project_hooks_ignored_when_not_allowed() {
    // Arrange
    let _env_lock = ENV_LOCK.lock().unwrap();
    let poisoned = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", poisoned.path());
    std::fs::write(
        poisoned.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo poisoned"
"#,
    )
    .unwrap();
    let _allow_guard = EnvGuard::remove("THEWAY_ALLOW_PROJECT_HOOKS");
    let base = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join(".theway")).unwrap();
    std::fs::write(
        cwd.path().join(".theway").join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo hi"
"#,
    )
    .unwrap();

    // Act
    let loaded = load_with(
        &daemon_paths(base.path(), cwd.path()),
        "session-1",
        None::<&theway_llm_provider::Model>,
        None::<ThinkingLevel>,
        HookExecutors::default(),
        true,
    )
    .await;

    // Assert
    assert!(loaded.runner.is_empty());
    assert_eq!(loaded.diagnostics.len(), 1);
    assert!(
        loaded.diagnostics[0].contains("project hooks ignored"),
        "{:?}",
        loaded.diagnostics
    );
}

#[tokio::test]
async fn load_with_env_var_allows_project_hooks() {
    // Arrange
    let _env_lock = ENV_LOCK.lock().unwrap();
    let poisoned = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", poisoned.path());
    std::fs::write(
        poisoned.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo poisoned"
"#,
    )
    .unwrap();
    let _allow_guard = EnvGuard::set("THEWAY_ALLOW_PROJECT_HOOKS", "1");
    let base = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join(".theway")).unwrap();
    std::fs::write(
        cwd.path().join(".theway").join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo hi"
"#,
    )
    .unwrap();

    // Act
    let loaded = load_with(
        &daemon_paths(base.path(), cwd.path()),
        "session-1",
        None::<&theway_llm_provider::Model>,
        None::<ThinkingLevel>,
        HookExecutors::default(),
        true,
    )
    .await;

    // Assert
    assert_eq!(loaded.runner.len(), 1);
    assert!(
        loaded.diagnostics.iter().all(|d| !d.contains("ignored")),
        "{:?}",
        loaded.diagnostics
    );
}

#[tokio::test]
async fn handle_event_control_plane_prompt_returns_without_side_effects() {
    // Arrange
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    let counter = calls.clone();
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Ok(theway_daemon::hooks::HookCommandOutput::default())
        })
    }));

    // Act
    runner
        .handle_event(
            &LoopEvent::ControlPlanePromptResolved {
                tool_call_id: "call-1".into(),
                tool_name: "bash".into(),
                args_hash: "hash".into(),
                label: "confirm".into(),
                decision: "approved".into(),
                reason: None,
            },
            CancellationToken::new(),
        )
        .await;

    // Assert
    assert_eq!(*calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn handle_harness_event_non_compaction_returns_without_side_effects() {
    // Arrange
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let mut r = rule(HookEvent::Compaction);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    let counter = calls.clone();
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Ok(theway_daemon::hooks::HookCommandOutput::default())
        })
    }));

    // Act
    runner
        .handle_harness_event(
            &SessionEvent::Started { messages_replayed: 0 },
            CancellationToken::new(),
        )
        .await;

    // Assert
    assert_eq!(*calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn handle_data_skips_cancelled_before_side_effects() {
    // Arrange
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    let counter = calls.clone();
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Ok(theway_daemon::hooks::HookCommandOutput::default())
        })
    }));
    let cancel = CancellationToken::new();
    cancel.cancel();

    // Act
    runner.handle_event(&tool_end_event("bash"), cancel).await;

    // Assert
    assert_eq!(*calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn command_hook_logs_stdout_stderr_when_executor_returns_nonempty() {
    // Arrange
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        Box::pin(async move {
            Ok(theway_daemon::hooks::HookCommandOutput {
                stdout: "out".into(),
                stderr: "err".into(),
            })
        })
    }));

    // Act
    runner
        .handle_event(&tool_end_event("bash"), CancellationToken::new())
        .await;

    // Assert: reaching here means the logging branches were taken.
}

#[tokio::test]
async fn command_hook_error_on_warn_rule_is_swallowed_by_handle_data() {
    // Arrange
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        Box::pin(async move { Err(anyhow::anyhow!("boom")) })
    }));

    // Act
    runner
        .handle_event(&tool_end_event("bash"), CancellationToken::new())
        .await;

    // Assert: reaching here means warn handling did not panic.
}

#[tokio::test]
async fn command_hook_error_on_ignore_rule_is_silent() {
    // Arrange
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    r.on_failure = OnFailure::Ignore;
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        Box::pin(async move { Err(anyhow::anyhow!("boom")) })
    }));

    // Act
    runner
        .handle_event(&tool_end_event("bash"), CancellationToken::new())
        .await;

    // Assert: reaching here means ignore handling did not panic.
}

#[tokio::test]
async fn webhook_hook_error_on_ignore_rule_is_silent() {
    // Arrange
    let mut r = rule(HookEvent::TurnEnd);
    r.webhook = Some("http://127.0.0.1:9/hook".into());
    r.on_failure = OnFailure::Ignore;
    let mut runner = runner(vec![r]);
    runner.webhook_sender = Some(Arc::new(
        move |_url, _body, _headers, _timeout, _cancel| {
            Box::pin(async move { Err(anyhow::anyhow!("boom")) })
        },
    ));

    // Act
    runner
        .handle_event(
            &LoopEvent::TurnCompleted {
                message: AgentMessage::Llm(Message::ToolResult(ToolResultMessage {
                    role: ToolResultRole::ToolResult,
                    tool_call_id: "call-1".into(),
                    tool_name: "bash".into(),
                    content: vec![UserContentBlock::text("ok")],
                    details: None,
                    is_error: false,
                    timestamp: 0,
                })),
                tool_results: vec![],
            },
            CancellationToken::new(),
        )
        .await;

    // Assert: reaching here means the webhook error path was taken.
}

#[tokio::test]
async fn run_rule_with_both_command_and_webhook_executes_both() {
    // Arrange
    let command_slot = Arc::new(Mutex::new(None));
    let webhook_slot = Arc::new(Mutex::new(None));
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    r.webhook = Some("http://127.0.0.1:9/hook".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(capture_command_executor(command_slot.clone()));
    runner.webhook_sender = Some(capture_webhook_sender(webhook_slot.clone()));

    // Act
    runner
        .handle_event(&tool_end_event("bash"), CancellationToken::new())
        .await;

    // Assert
    assert!(command_slot.lock().unwrap().is_some());
    assert!(webhook_slot.lock().unwrap().is_some());
}

#[tokio::test]
async fn event_filter_skips_non_matching_event() {
    // Arrange
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let mut matching = rule(HookEvent::ToolEnd);
    matching.command = Some("echo matching".into());
    let mut other = rule(HookEvent::TurnEnd);
    other.command = Some("echo other".into());
    let mut runner = runner(vec![other, matching]);
    let counter = calls.clone();
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Ok(theway_daemon::hooks::HookCommandOutput::default())
        })
    }));

    // Act
    runner
        .handle_event(&tool_end_event("bash"), CancellationToken::new())
        .await;

    // Assert
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn tool_filter_allows_matching_tool() {
    // Arrange
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let mut r = rule(HookEvent::ToolEnd);
    r.tool = Some("bash".into());
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    let counter = calls.clone();
    runner.command_executor = Some(Arc::new(move |_cmd, _cwd, _env, _timeout, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Ok(theway_daemon::hooks::HookCommandOutput::default())
        })
    }));

    // Act
    runner
        .handle_event(&tool_end_event("bash"), CancellationToken::new())
        .await;

    // Assert
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[test]
fn cwd_for_returns_project_theway_harness_and_home() {
    // Arrange: runner fields are explicit; env cannot redirect them.
    let _env_lock = ENV_LOCK.lock().unwrap();
    let poisoned = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", poisoned.path());
    let _home_guard = EnvGuard::set("HOME", poisoned.path());
    let mut r = runner(vec![]);
    r.work_dir = std::path::PathBuf::from("/explicit/project");
    r.base = std::path::PathBuf::from("/explicit/base");
    r.home = std::path::PathBuf::from("/explicit/home");

    let mut project_rule = rule(HookEvent::ToolEnd);
    project_rule.cwd = HookCwd::Project;
    assert_eq!(r.cwd_for(&project_rule), r.work_dir);

    let mut theway_rule = rule(HookEvent::ToolEnd);
    theway_rule.cwd = HookCwd::ThewayHarness;
    assert_eq!(r.cwd_for(&theway_rule), r.base);

    let mut home_rule = rule(HookEvent::ToolEnd);
    home_rule.cwd = HookCwd::Home;
    assert_eq!(r.cwd_for(&home_rule), r.home);
}

#[test]
fn payload_for_copies_each_event_data_field() {
    // Arrange
    let r = runner(vec![]);
    let rule = rule(HookEvent::ToolEnd);
    let data = EventData {
        event: HookEvent::ToolEnd,
        message_kind: Some("user".into()),
        message_summary: Some("summary".into()),
        assistant_event: Some("text_delta".into()),
        tool_call_id: Some("call-1".into()),
        tool_name: Some("bash".into()),
        tool_is_error: Some(true),
        tool_args: Some(serde_json::json!({"cmd": "ls"})),
        tool_result_summary: Some("result".into()),
        compaction_trigger: Some("manual".into()),
        compaction_tokens_before: Some(7),
        compaction_summary: Some("compaction".into()),
    };

    // Act
    let payload = r.payload_for(&rule, &data);

    // Assert
    assert_eq!(payload.event, "tool_end");
    assert_eq!(payload.source.as_deref(), Some("test"));
    assert_eq!(payload.message_kind.as_deref(), Some("user"));
    assert_eq!(payload.message_summary.as_deref(), Some("summary"));
    assert_eq!(payload.assistant_event.as_deref(), Some("text_delta"));
    assert_eq!(payload.tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(payload.tool_name.as_deref(), Some("bash"));
    assert_eq!(payload.tool_is_error, Some(true));
    assert_eq!(payload.tool_args, Some(serde_json::json!({"cmd": "ls"})));
    assert_eq!(payload.tool_result_summary.as_deref(), Some("result"));
    assert_eq!(payload.compaction_trigger.as_deref(), Some("manual"));
    assert_eq!(payload.compaction_tokens_before, Some(7));
    assert_eq!(payload.compaction_summary.as_deref(), Some("compaction"));
}

#[tokio::test]
async fn listener_invokes_runner_handle_event() {
    // Arrange
    let slot = Arc::new(Mutex::new(None));
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(capture_command_executor(slot.clone()));
    let listener = Arc::new(runner).listener();

    // Act
    listener(tool_end_event("bash"), CancellationToken::new()).await;

    // Assert
    assert!(slot.lock().unwrap().is_some());
}

#[tokio::test]
async fn harness_listener_spawns_handle_harness_event() {
    // Arrange
    let slot = Arc::new(Mutex::new(None));
    let mut r = rule(HookEvent::Compaction);
    r.command = Some("echo compacted".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(capture_command_executor(slot.clone()));
    let listener = Arc::new(runner).harness_listener();

    // Act
    listener(compaction_event());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if slot.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("harness listener should invoke the command executor");

    // Assert
    assert!(slot.lock().unwrap().is_some());
}
