// ── shared slash-command framework and pure helpers ─────────────────

use crate::commands::{
    CommandCtx, CommandOutcome, Registry, SlashCommand, attach_skill_prompt, cli_model_help_text,
    model_help_summary_lines, model_groups, parse, parse_model_spec, provider_summary,
};

struct TestCommand;

#[async_trait::async_trait]
impl SlashCommand for TestCommand {
    fn name(&self) -> &'static str {
        "test"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["t"]
    }

    fn description(&self) -> &'static str {
        "test command"
    }

    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, ()>) -> CommandOutcome {
        CommandOutcome::Handled
    }
}

#[test]
fn commands_parse_splits_quoted_argv() {
    assert_eq!(parse("hello"), None);
    assert_eq!(parse("   /"), None);
    assert_eq!(parse("/cmd").unwrap(), ("cmd".to_string(), Vec::new()));
    assert_eq!(
        parse("/cmd arg1 \"arg two\"\targ3").unwrap(),
        (
            "cmd".to_string(),
            vec!["arg1".to_string(), "arg two".to_string(), "arg3".to_string()]
        )
    );
    assert_eq!(
        parse("/cmd \"unterminated").unwrap(),
        ("cmd".to_string(), vec!["unterminated".to_string()])
    );
}

#[test]
fn commands_parse_model_spec_accepts_all_separators_and_rejects_empty() {
    assert_eq!(parse_model_spec("anthropic:claude-x"), Some(("anthropic", "claude-x")));
    assert_eq!(parse_model_spec(" anthropic/claude-x "), Some(("anthropic", "claude-x")));
    assert_eq!(parse_model_spec("anthropic claude-x"), Some(("anthropic", "claude-x")));
    assert_eq!(parse_model_spec("anthropic"), None);
    assert_eq!(parse_model_spec(":claude-x"), None);
    assert_eq!(parse_model_spec("anthropic:"), None);
    assert_eq!(parse_model_spec("  "), None);
}

#[test]
fn commands_attach_skill_prompt_passes_through_without_skill() {
    assert_eq!(attach_skill_prompt("hello", None), "hello");
    let with_skill = attach_skill_prompt("hello", Some("git-flow"));
    assert!(with_skill.starts_with("Before answering, invoke the Skill tool with name \"git-flow\""));
    assert!(with_skill.ends_with("User request:\nhello"));
}

#[test]
fn commands_registry_finds_by_name_alias_and_lists_names() {
    let mut registry = Registry::<()>::new();
    assert!(registry.find("test").is_none());
    registry.register(std::sync::Arc::new(TestCommand));
    assert!(registry.find("test").is_some());
    assert!(registry.find("t").is_some());
    assert!(registry.find("missing").is_none());
    assert_eq!(registry.names(), vec!["test", "t"]);
    assert_eq!(registry.commands().len(), 1);
}

#[test]
fn commands_model_helpers_produce_nonempty_summaries() {
    let groups = model_groups();
    let summary = provider_summary(&groups);
    assert!(!summary.is_empty());
    let help = model_help_summary_lines();
    assert!(help.len() >= 5);
    let cli = cli_model_help_text();
    assert!(cli.contains("Model catalog:"));
    assert!(cli.contains("Supported providers"));
}

#[test]
fn commands_outcome_debug_formats_variants() {
    let cases = [
        format!("{:?}", CommandOutcome::Handled),
        format!("{:?}", CommandOutcome::Quit),
        format!("{:?}", CommandOutcome::ClearScreen),
        format!("{:?}", CommandOutcome::Error("boom".into())),
        format!("{:?}", CommandOutcome::AttachSkill { name: "x".into() }),
        format!("{:?}", CommandOutcome::OpenModelPicker),
        format!("{:?}", CommandOutcome::WebRelay(crate::commands::WebRelayAction::Connect)),
        format!(
            "{:?}",
            CommandOutcome::SessionImportActivation {
                session_path: std::path::PathBuf::from("/tmp/s"),
                trigger_ids: vec!["tr".into()],
                cron_ids: vec!["cr".into()],
            }
        ),
    ];
    for case in cases {
        assert!(!case.is_empty());
    }
}

static CONSOLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn commands_console_sink_routes_emit_line() {
    let _guard = CONSOLE_LOCK.lock().unwrap();
    crate::commands::console::clear_sink();
    let captured: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_captured = captured.clone();
    crate::commands::console::set_sink(Box::new(move |line| {
        sink_captured.lock().unwrap().push(line);
    }));
    crate::commands::console::emit_line("routed".into());
    crate::commands::console::clear_sink();
    assert_eq!(*captured.lock().unwrap(), vec!["routed"]);
}
