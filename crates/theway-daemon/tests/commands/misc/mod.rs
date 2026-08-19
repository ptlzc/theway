//! Tests for `commands::misc` — split out of src (see docs/rust-test-files.md).

use super::*;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, PromptTemplate, Session,
    SessionStorage, Skill, SkillSource, ThinkingLevel,
};
use theway_llm_provider::{
    Api, AssistantMessage, AssistantRole, ContentBlock, Message, Provider, UserContent,
    UserContentBlock, UserMessage, UserRole,
};
use theway_transport::commands::{CommandCtx, CommandOutcome, WebRelayAction, console};

use crate::commands::DaemonCtx;
use theway_daemon::runtime_storage::local_runtime_storage;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;

static CONSOLE_LOCK: Mutex<()> = Mutex::new(());

struct ConsoleCapture {
    lines: Arc<Mutex<Vec<String>>>,
    _guard: MutexGuard<'static, ()>,
}

impl ConsoleCapture {
    fn start() -> Self {
        let guard = CONSOLE_LOCK.lock().unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink_lines = Arc::clone(&lines);
        console::set_sink(Box::new(move |line: String| {
            sink_lines.lock().unwrap().push(line);
        }));
        Self {
            lines,
            _guard: guard,
        }
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Drop for ConsoleCapture {
    fn drop(&mut self) {
        console::clear_sink();
    }
}

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
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

fn new_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn harness_with(session: Session) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
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

fn daemon_ctx(harness: &Arc<AgentHarness>, executor: Arc<TriggerExecutor>) -> DaemonCtx {
    DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor,
        storage: local_runtime_storage(),
    }
}

fn command_ctx<'a>(
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

fn sample_skill(name: &str, description: &str) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        file_path: format!("/tmp/{name}.SKILL.md"),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /diag
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn diag_prints_model_thinking_tools_skills_cost_and_log() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let harness = harness_with(session);
    harness.agent().state().thinking_level = Some(ThinkingLevel::High);

    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("session.log");
    let ctx = CommandCtx {
        session_id: "sess-1",
        log_path: Some(&log_path),
        tool_count: 3,
        cwd: tmp.path(),
        extra: &extra,
    };

    let outcome = DiagCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let lines = capture.lines();
    let text = lines.join("\n");
    assert!(text.contains("Diagnostic snapshot:"), "{text}");
    assert!(text.contains("session       sess-1"), "{text}");
    assert!(text.contains("model         faux:faux"), "{text}");
    assert!(text.contains("thinking      high"), "{text}");
    assert!(text.contains("tools         3"), "{text}");
    assert!(text.contains("skills        0"), "{text}");
    assert!(text.contains("cost"), "{text}");
    assert!(text.contains(&log_path.display().to_string()), "{text}");
}

#[tokio::test]
async fn diag_handles_missing_model_and_thinking() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let harness = harness_with(session);
    {
        let mut state = harness.agent().state();
        state.model = None;
        state.thinking_level = None;
    }

    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = DiagCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("model         (none)"), "{text}");
    assert!(text.contains("thinking      ?"), "{text}");
    assert!(text.contains("log file      (logging disabled)"), "{text}");
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /template
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn template_lists_empty_catalog_message() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("(no templates loaded"), "{text}");
}

#[tokio::test]
async fn template_lists_loaded_templates() {
    let capture = ConsoleCapture::start();
    let session = new_session();
    let options = AgentHarnessOptions {
        prompt_templates: vec![
            PromptTemplate {
                name: "greet".into(),
                description: Some("say hello".into()),
                content: "hello {{who}}".into(),
                file_path: "/tmp/greet.md".into(),
            },
            PromptTemplate {
                name: "plain".into(),
                description: None,
                content: "plain body".into(),
                file_path: "/tmp/plain.md".into(),
            },
        ],
        ..AgentHarnessOptions::new(faux_model(), session)
    };
    let harness = Arc::new(AgentHarness::new(options));
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("Loaded templates (2):"), "{text}");
    assert!(text.contains("/template greet  say hello"), "{text}");
    assert!(text.contains("/template plain  "), "{text}");
}

#[tokio::test]
async fn template_with_name_returns_run_prompt_template() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand
        .run(&["greet".into(), "who=world".into(), "x=1".into()], &ctx)
        .await;

    match outcome {
        CommandOutcome::RunPromptTemplate { name, vars } => {
            assert_eq!(name, "greet");
            assert_eq!(vars.get("who").and_then(|v| v.as_str()), Some("world"));
            assert_eq!(vars.get("x").and_then(|v| v.as_str()), Some("1"));
        }
        other => panic!("expected RunPromptTemplate, got {other:?}"),
    }
}

#[tokio::test]
async fn template_rejects_arg_without_equals() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TemplateCommand
        .run(&["greet".into(), "badarg".into()], &ctx)
        .await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("expected k=v argument; got: badarg"))
    );
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /compact
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn compact_without_args_uses_no_custom_instructions() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CompactCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::RunCompaction { custom: None }));
}

#[tokio::test]
async fn compact_joins_args_into_custom_instructions() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CompactCommand
        .run(&["keep".into(), "the".into(), "details".into()], &ctx)
        .await;

    assert!(
        matches!(outcome, CommandOutcome::RunCompaction { custom: Some(ref text) } if text == "keep the details")
    );
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /bug-report
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bug_report_writes_redacted_dump_to_base_dir() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = BugReportCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("wrote bug report:"), "{text}");
    let reports_dir = tmp.path().join("bug-reports");
    let written = std::fs::read_dir(reports_dir)
        .unwrap()
        .flatten()
        .count();
    assert_eq!(written, 1, "expected one bug report");
}

#[tokio::test]
async fn bug_report_returns_error_when_dest_cannot_be_created() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let file_as_base = tmp.path().join("not-a-dir");
    std::fs::write(&file_as_base, "x").unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", &file_as_base);

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = BugReportCommand.run(&[], &ctx).await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("bug-report failed:"))
    );
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /web-connect, /web-disconnect
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn web_connect_dispatches_connect_status_and_rejects_unknown() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    assert!(matches!(
        WebConnectCommand.run(&[], &ctx).await,
        CommandOutcome::WebRelay(WebRelayAction::Connect)
    ));
    assert!(matches!(
        WebConnectCommand.run(&["status".into()], &ctx).await,
        CommandOutcome::WebRelay(WebRelayAction::Status)
    ));
    assert!(
        matches!(
            WebConnectCommand.run(&["bogus".into()], &ctx).await,
            CommandOutcome::Error(ref msg) if msg.contains("unknown /web-connect argument: bogus")
        )
    );
}

#[tokio::test]
async fn web_disconnect_returns_relay_disconnect() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    assert!(matches!(
        WebDisconnectCommand.run(&[], &ctx).await,
        CommandOutcome::WebRelay(WebRelayAction::Disconnect)
    ));
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /find
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn find_without_query_returns_usage_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = FindCommand.run(&[], &ctx).await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /find <query>"))
    );
}

#[tokio::test]
async fn find_searches_session_repo_messages() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    // Arrange: create one session in the cwd repo with user text, user blocks,
    // and assistant text that contain (and miss) the query.
    let repo = theway_storage::session::open_repo(tmp.path()).await;
    let store = theway_storage::session::create(&repo, tmp.path())
        .await
        .unwrap();
    let session = theway_core::Session::from_store(Arc::new(store));
    session
        .append_message(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("needle user text".into()),
            timestamp: 0,
        })))
        .await
        .unwrap();
    session
        .append_message(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Blocks(vec![
                UserContentBlock::text("block needle text"),
                UserContentBlock::Image(theway_llm_provider::ImageContent {
                    data: "b64".into(),
                    mime_type: "image/png".into(),
                }),
            ]),
            timestamp: 0,
        })))
        .await
        .unwrap();
    session
        .append_message(AgentMessage::Llm(Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("assistant needle reply")],
            api: Api::from("faux"),
            provider: Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: theway_llm_provider::Usage::default(),
            stop_reason: theway_llm_provider::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })))
        .await
        .unwrap();
    drop(session);

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = FindCommand.run(&["needle".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("needle user text"), "{text}");
    assert!(text.contains("block needle text"), "{text}");
    assert!(text.contains("assistant needle reply"), "{text}");
    assert!(text.contains("(3 match(es))"), "{text}");

    let outcome = FindCommand.run(&["absent".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("(no matches)"), "{text}");
}

// ───────────────────────────────────────────────────────────────────────────────────────
// /history
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn history_prints_empty_notice_when_no_store() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = HistoryCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("(no history yet)"), "{text}");
}

#[tokio::test]
async fn history_lists_tail_with_limit_and_truncates_long_entries() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let mut store = theway_transport::history::HistoryStore::load();
    store.append("first prompt");
    store.append("second prompt");
    store.append(&"x".repeat(250));

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = HistoryCommand.run(&["1".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("3: "), "{text}");
    assert!(text.contains("…"), "{text}");
    assert!(!text.contains("first prompt"), "{text}");

    let outcome = HistoryCommand.run(&["not-a-number".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("1: first prompt"), "{text}");
    assert!(text.contains("2: second prompt"), "{text}");
    assert!(text.contains("3: "), "{text}");
}

// ───────────────────────────────────────────────────────────────────────────────────────
// help builders
// ───────────────────────────────────────────────────────────────────────────────────────

#[test]
fn help_text_with_skills_renders_skill_shortcuts_and_descriptions() {
    let registry = Registry::with_builtins();
    let skills = vec![sample_skill(
        "review-pr",
        "Review a pull request thoroughly",
    )];

    let help = help_text_with_skills(&registry, None, &skills);

    assert!(help.contains("Skill commands:"), "{help}");
    assert!(help.contains("/review-pr [prompt]"), "{help}");
    assert!(help.contains("Review a pull request thoroughly"), "{help}");
    assert!(help.contains("use loaded skill (user)"), "{help}");
}

#[test]
fn help_topic_resolves_skill_shortcut_by_name() {
    let registry = Registry::with_builtins();
    let skills = vec![sample_skill(
        "review-pr",
        "Review a pull request thoroughly",
    )];

    let help = help_text_with_skills(&registry, Some("/review-pr"), &skills);

    assert!(help.contains("/review-pr [prompt]"), "{help}");
    assert!(help.contains("use loaded skill 'review-pr'"), "{help}");
    assert!(help.contains("equivalent: /skill review-pr"), "{help}");
}

#[test]
fn help_topic_models_returns_catalog_text() {
    let registry = Registry::with_builtins();
    let help = help_text_with_skills(&registry, Some("models"), &[]);
    assert!(help.contains("Supported providers"), "{help}");
}

#[test]
fn general_help_lists_commands_with_usage_and_aliases() {
    let registry = Registry::with_builtins();
    let help = help_text(&registry, None);
    assert!(help.contains("Commands:"), "{help}");
    assert!(help.contains("/login"), "{help}");
    assert!(help.contains("/triggers"), "{help}");
    assert!(help.contains("Anything else is sent as a prompt to the agent."), "{help}");
}

#[test]
fn command_help_for_unknown_topic_suggests_skill_shortcut_prefix() {
    let registry = Registry::with_builtins();
    let skills = vec![sample_skill("daily-digest", "Daily digest")];
    let help = help_text_with_skills(&registry, Some("daily"), &skills);
    assert!(help.contains("unknown help topic: daily"), "{help}");
    assert!(help.contains("Did you mean /daily-digest?"), "{help}");
}
