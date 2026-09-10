//! `/help` text builders — skills shortcuts, topics, unknown topics, and usage lines.

use super::*;
use std::sync::Arc;

use theway_transport::commands::{CommandCtx, CommandOutcome};

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

/// Issue #73: `/reload` reconnects the provisioned MCP servers from the
/// stored configs — a fixed server list (or auth.json) takes effect without
/// a daemon restart, and failures land in the slot's errors for the panel.
#[test]
fn command_help_text_help_topic_shows_examples_line() {
    let registry = Registry::with_builtins();
    // The daemon registry doesn't register a `/help` command, so create a
    // tiny registry that does to cover the command_help_text `help` branch.
    let mut custom = crate::commands::Registry::new();
    struct HelpCommand;
    #[async_trait::async_trait]
    impl SlashCommand<crate::commands::DaemonCtx> for HelpCommand {
        fn name(&self) -> &'static str { "help" }
        fn description(&self) -> &'static str { "show help" }
        fn usage(&self) -> &'static str { "[topic]" }
        async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, crate::commands::DaemonCtx>) -> CommandOutcome {
            CommandOutcome::Handled
        }
    }
    custom.register(Arc::new(HelpCommand));
    let help = command_help_text(&custom, "help", &[]);
    assert!(help.contains("examples: /help model"), "{help}");
    drop(registry);
}

#[test]
fn command_help_text_unknown_topic_without_suggestions_gives_generic_hint() {
    let registry = Registry::with_builtins();
    let help = command_help_text(&registry, "zzzz-no-such-topic", &[]);
    assert!(help.contains("unknown help topic: zzzz-no-such-topic"), "{help}");
    assert!(help.contains("Run /help to list commands"), "{help}");
}

#[test]
fn help_text_with_skills_empty_topic_string_renders_general_help() {
    let registry = Registry::with_builtins();
    let help = help_text_with_skills(&registry, Some("   "), &[]);
    assert!(help.contains("Commands:"), "{help}");
}

#[test]
fn general_help_with_skill_shortcut_empty_description() {
    let registry = Registry::with_builtins();
    let mut skill = sample_skill("no-desc", "");
    skill.description.clear();
    let help = help_text_with_skills(&registry, None, &[skill]);
    assert!(help.contains("/no-desc [prompt]"), "{help}");
}

#[test]
fn skill_shortcuts_skip_disabled_and_duplicate_and_registry_names() {
    let registry = Registry::with_builtins();
    let mut disabled = sample_skill("disabled-skill", "desc");
    disabled.disable_model_invocation = true;
    let skills = vec![
        disabled,
        sample_skill("dup", "first"),
        sample_skill("dup", "second"),
        sample_skill("model", "shadow builtin"),
    ];
    let shortcuts = skill_shortcuts(&skills, &registry);
    assert!(
        !shortcuts.iter().any(|s| s.command == "/disabled-skill"),
        "{shortcuts:?}"
    );
    assert!(
        !shortcuts.iter().any(|s| s.command == "/dup"),
        "{shortcuts:?}"
    );
    assert!(
        !shortcuts.iter().any(|s| s.command == "/model"),
        "{shortcuts:?}"
    );
}
#[test]
fn command_help_text_handles_empty_usage_and_skill_without_description() {
    let mut registry = crate::commands::Registry::new();
    struct EmptyUsageCommand;
    #[async_trait::async_trait]
    impl SlashCommand<crate::commands::DaemonCtx> for EmptyUsageCommand {
        fn name(&self) -> &'static str { "emptyusage" }
        fn description(&self) -> &'static str { "no usage" }
        fn usage(&self) -> &'static str { "" }
        async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, crate::commands::DaemonCtx>) -> CommandOutcome {
            CommandOutcome::Handled
        }
    }
    registry.register(Arc::new(EmptyUsageCommand));
    let help = command_help_text(&registry, "emptyusage", &[]);
    assert!(help.contains("/emptyusage"), "{help}");

    let mut skill = sample_skill("descript-less", "");
    skill.description.clear();
    let help = command_help_text(&registry, "descript-less", &[skill]);
    assert!(help.contains("/descript-less [prompt]"), "{help}");
}
