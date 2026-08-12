//! `/skill` — attach a loaded skill to the next prompt.
//! (`attach_skill_prompt` moved to the SDK — sdk-split-local-sandbox, node 5-commands-layer.)

use super::*;

use theway_sdk::commands::CommandCtx;

pub struct SkillCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for SkillCommand {
    fn name(&self) -> &'static str {
        "skill"
    }
    fn description(&self) -> &'static str {
        "attach a loaded skill to the next prompt"
    }
    fn usage(&self) -> &'static str {
        "<name>"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        if argv.len() != 1 {
            return CommandOutcome::Error("usage: /skill <name>".into());
        }
        let name = &argv[0];
        let skills = ctx.harness.skills();
        let Some(skill) = skills.iter().find(|s| s.name == *name) else {
            let mut matches = skills
                .iter()
                .filter(|s| s.name.starts_with(name))
                .map(|s| s.name.as_str())
                .take(5)
                .collect::<Vec<_>>();
            if matches.is_empty() {
                matches = skills
                    .iter()
                    .filter(|s| s.name.contains(name))
                    .map(|s| s.name.as_str())
                    .take(5)
                    .collect::<Vec<_>>();
            }
            let hint = if matches.is_empty() {
                "".to_string()
            } else {
                format!(" Did you mean: {}?", matches.join(", "))
            };
            return CommandOutcome::Error(format!(
                "no skill named '{name}'. Run /skills to list loaded skills.{hint}"
            ));
        };
        if skill.disable_model_invocation {
            return CommandOutcome::Error(format!(
                "skill '{name}' is disabled (disable_model_invocation=true); edit the skill frontmatter to enable it"
            ));
        }
        cprintln!(
            "using skill: {} ({}) for next turn",
            skill.name,
            skill.source.label()
        );
        CommandOutcome::AttachSkill { name: name.clone() }
    }
}
