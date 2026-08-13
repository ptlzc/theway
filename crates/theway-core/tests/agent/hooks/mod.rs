use super::*;
use std::sync::Mutex;
use theway_core::{AgentToolResult, LoopEvent, SessionEvent};
use theway_llm_provider::{ToolResultMessage, ToolResultRole, UserContentBlock};

/// Details of one command-executor invocation, captured by the fake seam.
#[derive(Debug)]
struct CommandCall {
    command: String,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    timeout: Duration,
    payload: serde_json::Value,
}

/// Details of one webhook-sender invocation, captured by the fake seam.
#[derive(Debug)]
struct WebhookCall {
    url: String,
    body: String,
    headers: BTreeMap<String, String>,
    timeout: Duration,
}

fn runner(rules: Vec<HookRule>) -> HookRunner {
    HookRunner {
        rules,
        session_id: "session-1".into(),
        cwd: std::env::current_dir().unwrap(),
        model_provider: "faux".into(),
        model_id: "model".into(),
        thinking_level: "off".into(),
        command_executor: None,
        webhook_sender: None,
    }
}

/// Fake command executor: reads the payload file the runner wrote (from the injected
/// `THEWAY_HOOK_PAYLOAD` env var) and records the full call into `slot`. The payload
/// file is still present at this point — the runner removes it after the executor
/// returns.
fn capture_command_executor(slot: Arc<Mutex<Option<CommandCall>>>) -> HookCommandExecutor {
    Arc::new(move |command, cwd, env, timeout, _cancel| {
        let slot = slot.clone();
        Box::pin(async move {
            let payload_path = env
                .get("THEWAY_HOOK_PAYLOAD")
                .map(PathBuf::from)
                .expect("runner must inject THEWAY_HOOK_PAYLOAD");
            let payload_text = tokio::fs::read_to_string(&payload_path)
                .await
                .expect("payload file must exist while the executor runs");
            let payload: serde_json::Value =
                serde_json::from_str(&payload_text).expect("payload file must be valid JSON");
            *slot.lock().unwrap() = Some(CommandCall {
                command,
                cwd,
                env,
                timeout,
                payload,
            });
            Ok(HookCommandOutput::default())
        })
    })
}

/// Fake webhook sender: records the full call into `slot`.
fn capture_webhook_sender(slot: Arc<Mutex<Option<WebhookCall>>>) -> HookWebhookSender {
    Arc::new(move |url, body, headers, timeout, _cancel| {
        let slot = slot.clone();
        Box::pin(async move {
            *slot.lock().unwrap() = Some(WebhookCall {
                url,
                body,
                headers,
                timeout,
            });
            Ok(())
        })
    })
}

fn rule(event: HookEvent) -> HookRule {
    HookRule {
        event,
        command: None,
        webhook: None,
        headers: BTreeMap::new(),
        timeout_ms: 1_000,
        cwd: HookCwd::Project,
        on_failure: OnFailure::Warn,
        tool: None,
        source: "test".into(),
    }
}

#[test]
fn parses_hook_rules_and_skips_bad_entries() {
    let file: HooksFile = toml::from_str(
        r#"
allow_project_hooks = true

[[hook]]
event = "tool_end"
command = "echo ok"
tool = "bash"

[[hook]]
event = "compaction"
command = "echo compacted"

[[hook]]
event = "not_real"
command = "echo nope"
            "#,
    )
    .unwrap();
    let mut rules = Vec::new();
    let mut diagnostics = Vec::new();
    push_rules(file, "test", &mut rules, &mut diagnostics);
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].event, HookEvent::ToolEnd);
    assert_eq!(rules[0].tool.as_deref(), Some("bash"));
    assert_eq!(rules[1].event, HookEvent::Compaction);
    assert_eq!(diagnostics.len(), 1);
}

#[tokio::test]
async fn command_hook_passes_env_and_payload_to_executor() {
    let slot: Arc<Mutex<Option<CommandCall>>> = Arc::new(Mutex::new(None));
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some("echo hi".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(capture_command_executor(slot.clone()));
    let ev = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        result: AgentToolResult {
            content: vec![UserContentBlock::text("ok")],
            details: serde_json::Value::Null,
            terminate: None,
        },
        is_error: false,
    };
    runner.handle_event(&ev, CancellationToken::new()).await;

    let call = slot.lock().unwrap().take().expect("executor was not called");
    assert_eq!(call.command, "echo hi");
    assert_eq!(call.cwd, std::env::current_dir().unwrap());
    assert_eq!(call.timeout, Duration::from_millis(1_000));
    assert_eq!(call.env["THEWAY_HOOK_EVENT"], "tool_end");
    assert_eq!(call.env["THEWAY_TOOL_NAME"], "bash");
    assert_eq!(call.env["THEWAY_SESSION_ID"], "session-1");
    assert_eq!(call.payload["event"], "tool_end");
    assert_eq!(call.payload["tool_name"], "bash");

    // The payload file must have been cleaned up after the executor returned.
    let payload_path = PathBuf::from(&call.env["THEWAY_HOOK_PAYLOAD"]);
    assert!(
        !payload_path.exists(),
        "payload file must be removed after the run: {}",
        payload_path.display()
    );
}

#[tokio::test]
async fn compaction_command_hook_passes_env_and_payload_to_executor() {
    let slot: Arc<Mutex<Option<CommandCall>>> = Arc::new(Mutex::new(None));
    let mut r = rule(HookEvent::Compaction);
    r.command = Some("echo compacted".into());
    let mut runner = runner(vec![r]);
    runner.command_executor = Some(capture_command_executor(slot.clone()));
    let ev = SessionEvent::Compaction {
        from_hook: true,
        summary: "summary text".into(),
        tokens_before: 42,
    };
    runner
        .handle_harness_event(&ev, CancellationToken::new())
        .await;

    let call = slot.lock().unwrap().take().expect("executor was not called");
    assert_eq!(call.env["THEWAY_HOOK_EVENT"], "compaction");
    assert_eq!(call.env["THEWAY_COMPACTION_TRIGGER"], "manual");
    assert_eq!(call.env["THEWAY_COMPACTION_TOKENS_BEFORE"], "42");
    assert_eq!(call.payload["compaction_summary"], "summary text");
    assert_eq!(call.payload["compaction_trigger"], "manual");
    assert_eq!(call.payload["compaction_tokens_before"].as_u64(), Some(42));
}

#[tokio::test]
async fn webhook_hook_passes_payload_to_sender() {
    let slot: Arc<Mutex<Option<WebhookCall>>> = Arc::new(Mutex::new(None));
    let mut r = rule(HookEvent::TurnEnd);
    r.webhook = Some("http://127.0.0.1:9/hook".into());
    r.headers.insert("X-Test".into(), "v".into());
    let mut runner = runner(vec![r]);
    runner.webhook_sender = Some(capture_webhook_sender(slot.clone()));
    let ev = LoopEvent::TurnCompleted {
        message: AgentMessage::Llm(theway_llm_provider::Message::ToolResult(
            ToolResultMessage {
                role: ToolResultRole::ToolResult,
                tool_call_id: "call-1".into(),
                tool_name: "bash".into(),
                content: vec![UserContentBlock::text("ok")],
                details: None,
                is_error: false,
                timestamp: 0,
            },
        )),
        tool_results: Vec::new(),
    };
    runner.handle_event(&ev, CancellationToken::new()).await;

    let call = slot.lock().unwrap().take().expect("sender was not called");
    assert_eq!(call.url, "http://127.0.0.1:9/hook");
    assert_eq!(call.headers.get("X-Test").map(String::as_str), Some("v"));
    assert_eq!(call.timeout, Duration::from_millis(1_000));
    let payload: serde_json::Value = serde_json::from_str(&call.body).unwrap();
    assert_eq!(payload["event"], "turn_end");
    assert_eq!(payload["session_id"], "session-1");
}

#[tokio::test]
async fn tool_filter_skips_non_matching_tool() {
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let mut r = rule(HookEvent::ToolEnd);
    r.tool = Some("bash".into());
    r.command = Some("touch whatever".into());
    let mut runner = runner(vec![r]);
    let counter = calls.clone();
    runner.command_executor = Some(Arc::new(move |_command, _cwd, _env, _timeout, _cancel| {
        let counter = counter.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Ok(HookCommandOutput::default())
        })
    }));
    let ev = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        result: AgentToolResult::default(),
        is_error: false,
    };
    runner.handle_event(&ev, CancellationToken::new()).await;
    assert_eq!(*calls.lock().unwrap(), 0);
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    old: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let old = std::env::var(key).ok();
        // Tests in Rust 2024 require acknowledging that process env is global.
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(old) = &self.old {
            unsafe { std::env::set_var(self.key, old) };
        } else {
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

/// Without injected executors the loader must report the degraded mode in
/// diagnostics, and handling events with side-effect rules must skip the side
/// effects (core is diagnostics-only without a host).
#[tokio::test]
async fn load_without_executors_reports_skip_diagnostics() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let theway_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", theway_dir.path());

    std::fs::write(
        theway_dir.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo hi"
webhook = "http://127.0.0.1:9/hook"
"#,
    )
    .unwrap();

    let loaded = load(
        cwd.path(),
        "session-no-executors",
        None::<&theway_llm_provider::Model>,
        None::<ThinkingLevel>,
        HookExecutors::default(),
    )
    .await;

    assert_eq!(loaded.runner.len(), 1);
    assert_eq!(loaded.diagnostics.len(), 2);
    assert!(
        loaded.diagnostics[0].contains("no command executor"),
        "unexpected diagnostics: {:?}",
        loaded.diagnostics
    );
    assert!(
        loaded.diagnostics[1].contains("no webhook sender"),
        "unexpected diagnostics: {:?}",
        loaded.diagnostics
    );

    // Handling a matching event must not panic and must not perform side effects —
    // the runner simply skips both sides.
    let ev = LoopEvent::TurnCompleted {
        message: AgentMessage::Llm(theway_llm_provider::Message::ToolResult(
            ToolResultMessage {
                role: ToolResultRole::ToolResult,
                tool_call_id: "call-1".into(),
                tool_name: "bash".into(),
                content: vec![UserContentBlock::text("ok")],
                details: None,
                is_error: false,
                timestamp: 0,
            },
        )),
        tool_results: Vec::new(),
    };
    loaded
        .runner
        .handle_event(&ev, CancellationToken::new())
        .await;
}
