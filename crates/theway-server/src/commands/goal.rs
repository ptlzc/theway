//! `/goal` and `/goal-start` — session goal stop-hook controls.

use super::*;

use theway_sdk::commands::CommandCtx;

pub struct GoalCommand;

fn goal_start_prompt(argv: &[String]) -> String {
    argv.join(" ").trim().to_string()
}

async fn run_goal_start(prompt: String, ctx: &CommandCtx<'_>) -> CommandOutcome {
    if prompt.is_empty() {
        return CommandOutcome::Error("usage: /goal-start <prompt>".into());
    }
    if let Err(e) = theway_core::multiagent::goal::current(ctx.harness)
        .await
        .filter(|state| state.active())
        .ok_or_else(|| "no active goal; set one with /goal <condition>".to_string())
    {
        return CommandOutcome::Error(e);
    }
    CommandOutcome::RunAgentPrompt {
        prompt,
        error_context: "goal start: ",
    }
}

#[async_trait]
impl SlashCommand for GoalCommand {
    fn name(&self) -> &'static str {
        "goal"
    }

    fn description(&self) -> &'static str {
        "set, view, pause, resume, or clear the session goal stop hook"
    }

    fn usage(&self) -> &'static str {
        "[<condition>|start <prompt>|pause|resume|clear]"
    }

    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        match argv.first().map(String::as_str) {
            None => {
                print_goal_status(ctx).await;
                CommandOutcome::Handled
            }
            Some("pause") if argv.len() == 1 => {
                match theway_core::multiagent::goal::pause(ctx.harness).await {
                    Ok(state) => {
                        cprintln!("goal paused: {}", state.condition);
                        CommandOutcome::Handled
                    }
                    Err(e) => CommandOutcome::Error(e),
                }
            }
            Some("resume") if argv.len() == 1 => {
                match theway_core::multiagent::goal::resume(ctx.harness).await {
                    Ok(state) => {
                        cprintln!("goal resumed: {}", state.condition);
                        CommandOutcome::Handled
                    }
                    Err(e) => CommandOutcome::Error(e),
                }
            }
            Some("clear") if argv.len() == 1 => {
                match theway_core::multiagent::goal::clear(ctx.harness).await {
                    Ok(_) => {
                        cprintln!("goal cleared");
                        CommandOutcome::Handled
                    }
                    Err(e) => CommandOutcome::Error(e),
                }
            }
            Some("start") => run_goal_start(goal_start_prompt(&argv[1..]), ctx).await,
            Some(_) => {
                let condition = argv.join(" ").trim().to_string();
                if condition.is_empty() {
                    return CommandOutcome::Error("usage: /goal <condition>".into());
                }
                match theway_core::multiagent::goal::set(ctx.harness, condition).await {
                    Ok(state) => {
                        cprintln!("goal set: {}", state.condition);
                        cprintln!(
                            "goal will continue after each successful turn until transcript evidence satisfies the condition"
                        );
                        cprintln!("start by sending a normal prompt, or run /goal-start <prompt>");
                        CommandOutcome::Handled
                    }
                    Err(e) => CommandOutcome::Error(e),
                }
            }
        }
    }
}

pub struct GoalStartCommand;

#[async_trait]
impl SlashCommand for GoalStartCommand {
    fn name(&self) -> &'static str {
        "goal-start"
    }

    fn description(&self) -> &'static str {
        "start working on the active session goal with a prompt"
    }

    fn usage(&self) -> &'static str {
        "<prompt>"
    }

    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        run_goal_start(goal_start_prompt(argv), ctx).await
    }
}

async fn print_goal_status(ctx: &CommandCtx<'_>) {
    match theway_core::multiagent::goal::current(ctx.harness).await {
        Some(state)
            if state.active()
                || state.status == theway_core::multiagent::goal::GoalStatus::Achieved =>
        {
            cprintln!("goal: {}", state.condition);
            cprintln!("status: {}", state.status.as_str());
            cprintln!("iterations: {}", state.iterations);
            if let Some(reason) = state.last_reason.as_deref() {
                cprintln!("last evaluator reason: {}", preview_text(reason, 240));
            }
        }
        _ => {
            cprintln!("no active goal; set one with /goal <condition>");
        }
    }
}
