//! Automation commands: `/triggers`, `/new-trigger`, `/cron`, `/inbox`.

mod render;

use super::*;

use render::preview_cron_action;
pub(crate) use render::{render_cron_jobs, render_dynamic_trigger_rules, render_triggers_status};
// Audit-row helpers the `tests/commands/` mirror reaches through `use super::*` in mod.rs.
#[cfg(test)]
pub(in crate::commands) use render::trigger_decision_details;
pub(in crate::commands) use render::{
    collect_trigger_audit_rows, render_running_triggers, render_trigger_audit,
    render_trigger_sources,
};

pub struct TriggersCommand;

#[async_trait]
impl SlashCommand for TriggersCommand {
    fn name(&self) -> &'static str {
        "triggers"
    }
    fn description(&self) -> &'static str {
        "show trigger sources, rules, running actions, and recent audit"
    }
    fn usage(&self) -> &'static str {
        "[status|rules|sources|enable <id>|disable <id>|remove <id>|remove --all|running|audit [N]|abort <trace_id>|abort --all]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let subcommand = argv.first().map(String::as_str).unwrap_or("status");
        match subcommand {
            "status" => {
                let snapshot = ctx.trigger_executor.notification_status_snapshot();
                for line in render_triggers_status(&snapshot) {
                    cprintln!("{line}");
                }
                CommandOutcome::Handled
            }
            "rules" => {
                let rules = crate::triggers::global_registry().list();
                for line in render_dynamic_trigger_rules(&rules, usize::MAX) {
                    cprintln!("{line}");
                }
                if rules.is_empty()
                    && let Some(hint) = automation_elsewhere_hint_for_ctx(ctx).await
                {
                    cprintln!("{hint}");
                }
                CommandOutcome::Handled
            }
            "remove" | "rm" | "delete" => {
                let Some(target) = argv.get(1) else {
                    return CommandOutcome::Error("usage: /triggers remove <id>|--all".into());
                };
                if target == "--all" {
                    match crate::triggers::global_registry().clear_rules() {
                        Ok(count) => {
                            cprintln!("removed {count} dynamic trigger rule(s)");
                            CommandOutcome::Handled
                        }
                        Err(e) => CommandOutcome::Error(e.to_string()),
                    }
                } else {
                    match crate::triggers::global_registry().remove_rule(target) {
                        Ok(Some(rule)) => {
                            cprintln!("removed trigger {}", rule.id);
                            cprintln!("  condition: {}", rule.condition);
                            cprintln!("  action: {}", rule.action);
                            CommandOutcome::Handled
                        }
                        Ok(None) => CommandOutcome::Error(format!(
                            "no dynamic trigger rule with id '{target}'"
                        )),
                        Err(e) => CommandOutcome::Error(e.to_string()),
                    }
                }
            }
            "enable" | "resume" => set_dynamic_trigger_enabled(argv.get(1), true),
            "disable" | "pause" => set_dynamic_trigger_enabled(argv.get(1), false),
            "sources" | "hooks" => {
                let snapshot = ctx.trigger_executor.notification_status_snapshot();
                for line in render_trigger_sources(&snapshot.hooks) {
                    cprintln!("{line}");
                }
                CommandOutcome::Handled
            }
            "running" => {
                let snapshot = ctx.trigger_executor.notification_status_snapshot();
                for line in render_running_triggers(&snapshot.running) {
                    cprintln!("{line}");
                }
                CommandOutcome::Handled
            }
            "audit" => {
                let limit = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
                let entries = match ctx.harness.session().entries().await {
                    Ok(entries) => entries,
                    Err(e) => return CommandOutcome::Error(format!("read trigger audit: {e}")),
                };
                let rows = collect_trigger_audit_rows(&entries, limit);
                for line in render_trigger_audit(&rows) {
                    cprintln!("{line}");
                }
                CommandOutcome::Handled
            }
            "abort" => {
                let Some(target) = argv.get(1) else {
                    return CommandOutcome::Error("usage: /triggers abort <trace_id>|--all".into());
                };
                let snapshot = ctx.trigger_executor.notification_status_snapshot();
                if target == "--all" {
                    let count = snapshot.running.len();
                    ctx.trigger_executor.abort_all_triggers();
                    cprintln!("requested abort for {count} running trigger(s)");
                } else {
                    if !snapshot.running.iter().any(|t| t.trace_id == *target) {
                        return CommandOutcome::Error(format!(
                            "no running trigger with trace_id '{target}'"
                        ));
                    }
                    ctx.trigger_executor.abort_trigger(target);
                    cprintln!("requested abort for trigger {target}");
                }
                CommandOutcome::Handled
            }
            other => CommandOutcome::Error(format!(
                "unknown /triggers command: {other}. usage: /triggers {}",
                self.usage()
            )),
        }
    }
}

pub struct NewTriggerCommand;

#[async_trait]
impl SlashCommand for NewTriggerCommand {
    fn name(&self) -> &'static str {
        "new-trigger"
    }

    fn description(&self) -> &'static str {
        "create a dynamic natural-language trigger rule"
    }

    fn usage(&self) -> &'static str {
        "<natural-language trigger request>"
    }

    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_>) -> CommandOutcome {
        let spec = argv.join(" ");
        if spec.trim().is_empty() {
            return CommandOutcome::Error(
                "usage: /new-trigger <natural-language trigger request>".into(),
            );
        }

        let prompt = format!(
            "The user asked theway to create a dynamic trigger. Extract the trigger condition and action from the request, then call NewTrigger with structured condition and action fields. Dynamic triggers fire once by default; set fire_once=false only when the user explicitly asks for a repeating trigger. Trigger output is shown in the TUI and audit by default; set promote_to_chat=true only when the user explicitly asks for trigger results to enter the main chat context or be visible to future turns. Do not require a fixed syntax. If either the condition or action is missing, ask one concise clarification question instead of calling tools.\n\nUser request:\n{spec}"
        );
        CommandOutcome::RunAgentPrompt {
            prompt,
            error_context: "create trigger: ",
        }
    }
}

pub struct CronCommand;

#[async_trait]
impl SlashCommand for CronCommand {
    fn name(&self) -> &'static str {
        "cron"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["crontab"]
    }

    fn description(&self) -> &'static str {
        "manage local scheduled agent jobs"
    }

    fn usage(&self) -> &'static str {
        "[list|add \"<5-field-cron>\" <prompt>|enable <id>|disable <id>|remove <id>]"
    }

    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        let subcommand = argv.first().map(String::as_str).unwrap_or("list");
        match subcommand {
            "list" | "ls" | "status" => {
                let jobs = crate::triggers::global_cron_registry().list();
                for line in render_cron_jobs(&jobs) {
                    cprintln!("{line}");
                }
                if jobs.is_empty()
                    && let Some(hint) = automation_elsewhere_hint_for_ctx(ctx).await
                {
                    cprintln!("{hint}");
                }
                CommandOutcome::Handled
            }
            "add" => {
                let mut rest: Vec<&String> = argv[1..].iter().collect();
                let stateful = rest
                    .iter()
                    .position(|arg| arg.as_str() == "--stateful")
                    .map(|idx| {
                        rest.remove(idx);
                    })
                    .is_some();
                if rest.len() < 2 {
                    return CommandOutcome::Error(
                        "usage: /cron add [--stateful] \"<minute hour dom month dow>\" <prompt>"
                            .into(),
                    );
                }
                let schedule = rest[0];
                let action = rest[1..]
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                match crate::triggers::global_cron_registry()
                    .add_job_full(schedule, &action, stateful)
                {
                    Ok(job) => {
                        write_cron_control_plane_audit(ctx, "add", None, Some(&job)).await;
                        cprintln!("added cron job {}", job.id);
                        cprintln!("  schedule: {}", job.schedule);
                        if job.stateful {
                            cprintln!("  mode: stateful loop (findings go to /inbox)");
                        }
                        cprintln!("  action: {}", preview_cron_action(&job.action));
                        CommandOutcome::Handled
                    }
                    Err(e) => CommandOutcome::Error(e.to_string()),
                }
            }
            "enable" | "resume" => set_cron_enabled(ctx, argv.get(1), true).await,
            "disable" | "pause" => set_cron_enabled(ctx, argv.get(1), false).await,
            "remove" | "rm" | "delete" => {
                let Some(id) = argv.get(1) else {
                    return CommandOutcome::Error("usage: /cron remove <id>".into());
                };
                match crate::triggers::global_cron_registry().remove_job(id) {
                    Ok(Some(job)) => {
                        write_cron_control_plane_audit(ctx, "remove", Some(&job), None).await;
                        cprintln!("removed cron job {}", job.id);
                        CommandOutcome::Handled
                    }
                    Ok(None) => CommandOutcome::Error(format!("no cron job with id '{id}'")),
                    Err(e) => CommandOutcome::Error(e.to_string()),
                }
            }
            other => CommandOutcome::Error(format!(
                "unknown /cron command: {other}. usage: /cron {}",
                self.usage()
            )),
        }
    }
}

async fn set_cron_enabled(
    ctx: &CommandCtx<'_>,
    id: Option<&String>,
    enabled: bool,
) -> CommandOutcome {
    let Some(id) = id else {
        return CommandOutcome::Error(format!(
            "usage: /cron {} <id>",
            if enabled { "enable" } else { "disable" }
        ));
    };
    let before = crate::triggers::global_cron_registry()
        .list()
        .into_iter()
        .find(|job| job.id == *id);
    match crate::triggers::global_cron_registry().set_job_enabled(id, enabled) {
        Ok(Some(job)) => {
            write_cron_control_plane_audit(
                ctx,
                if enabled { "enable" } else { "disable" },
                before.as_ref(),
                Some(&job),
            )
            .await;
            cprintln!(
                "{} cron job {}",
                if enabled { "enabled" } else { "disabled" },
                job.id
            );
            CommandOutcome::Handled
        }
        Ok(None) => CommandOutcome::Error(format!("no cron job with id '{id}'")),
        Err(e) => CommandOutcome::Error(e.to_string()),
    }
}

async fn write_cron_control_plane_audit(
    ctx: &CommandCtx<'_>,
    op: &str,
    before: Option<&crate::triggers::cron::CronJob>,
    after: Option<&crate::triggers::cron::CronJob>,
) {
    let job = after.or(before);
    let audit = crate::triggers::cron::cron_control_plane_audit(op, "slash", before, after);
    if let Err(e) = ctx
        .harness
        .session()
        .append_custom("cron_control_plane", Some(audit))
        .await
    {
        tracing::warn!(
            op = %op,
            job_id = job.map(|job| job.id.as_str()).unwrap_or("<unknown>"),
            error = %e,
            "cron_control_plane audit write failed; slash cron change itself succeeded"
        );
    }
}

/// Hint at enabled automation living in sibling sessions of this cwd. Used by the empty
/// states of `/cron list` and `/triggers rules`, where "none" otherwise reads as data loss
/// when the user's jobs simply live in another session.
async fn automation_elsewhere_hint_for_ctx(ctx: &CommandCtx<'_>) -> Option<String> {
    let metadata = ctx
        .harness
        .session()
        .storage()
        .get_metadata_json()
        .await
        .ok()?;
    let current = metadata
        .get("path")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from);
    let repo = theway_sdk::session::open_repo(ctx.cwd).await;
    theway_sdk::session::automation_elsewhere_hint(&repo, current.as_deref()).await
}

pub struct InboxCommand;

#[async_trait]
impl SlashCommand for InboxCommand {
    fn name(&self) -> &'static str {
        "inbox"
    }
    fn description(&self) -> &'static str {
        "triage findings from loops (stateful cron jobs)"
    }
    fn usage(&self) -> &'static str {
        "[all|claim <id|n>|dismiss <id|n>|clear]"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_>) -> CommandOutcome {
        let path = theway_transport::inbox::default_inbox_path();
        match argv.first().map(String::as_str) {
            None | Some("list") => {
                let entries = match theway_transport::inbox::list_new(&path) {
                    Ok(entries) => entries,
                    Err(e) => return CommandOutcome::Error(format!("inbox: {e}")),
                };
                if entries.is_empty() {
                    cprintln!(
                        "inbox: empty — stateful loops (/cron add --stateful) report findings here"
                    );
                    return CommandOutcome::Handled;
                }
                cprintln!("Inbox ({} new):", entries.len());
                for (idx, entry) in entries.iter().enumerate() {
                    cprintln!(
                        "  {}. [{}] {}  ({}, {})",
                        idx + 1,
                        entry.id.chars().take(12).collect::<String>(),
                        entry.text,
                        entry.source,
                        entry.created_at.chars().take(16).collect::<String>()
                    );
                }
                cprintln!("claim with /inbox claim <n>, dismiss with /inbox dismiss <n>");
                CommandOutcome::Handled
            }
            Some("all") => {
                let entries = match theway_transport::inbox::list(&path) {
                    Ok(entries) => entries,
                    Err(e) => return CommandOutcome::Error(format!("inbox: {e}")),
                };
                cprintln!("Inbox history ({} total):", entries.len());
                for entry in &entries {
                    let status = match entry.status {
                        theway_transport::inbox::InboxStatus::New => "new",
                        theway_transport::inbox::InboxStatus::Claimed => "claimed",
                        theway_transport::inbox::InboxStatus::Dismissed => "dismissed",
                    };
                    cprintln!("  [{status}] {}  ({})", entry.text, entry.source);
                }
                CommandOutcome::Handled
            }
            Some("claim") => match resolve_inbox_target(&path, argv.get(1)) {
                Ok(entry) => {
                    if let Err(e) = theway_transport::inbox::set_status(
                        &path,
                        &entry.id,
                        theway_transport::inbox::InboxStatus::Claimed,
                    ) {
                        return CommandOutcome::Error(format!("inbox: {e}"));
                    }
                    CommandOutcome::RunAgentPrompt {
                        prompt: format!(
                            "A recurring loop ({}) reported this finding — investigate and address it:\n{}",
                            entry.source, entry.text
                        ),
                        error_context: "inbox claim",
                    }
                }
                Err(e) => CommandOutcome::Error(e),
            },
            Some("dismiss") => match resolve_inbox_target(&path, argv.get(1)) {
                Ok(entry) => {
                    match theway_transport::inbox::set_status(
                        &path,
                        &entry.id,
                        theway_transport::inbox::InboxStatus::Dismissed,
                    ) {
                        Ok(_) => {
                            cprintln!("dismissed: {}", entry.text);
                            CommandOutcome::Handled
                        }
                        Err(e) => CommandOutcome::Error(format!("inbox: {e}")),
                    }
                }
                Err(e) => CommandOutcome::Error(e),
            },
            Some("clear") => match theway_transport::inbox::dismiss_all_new(&path) {
                Ok(n) => {
                    cprintln!(
                        "dismissed {n} inbox entr{}",
                        if n == 1 { "y" } else { "ies" }
                    );
                    CommandOutcome::Handled
                }
                Err(e) => CommandOutcome::Error(format!("inbox: {e}")),
            },
            Some(other) => CommandOutcome::Error(format!(
                "unknown /inbox subcommand: {other}; usage: /inbox [all|claim <n>|dismiss <n>|clear]"
            )),
        }
    }
}

/// Resolve `<n>` (1-based position in the `new` list) or an `inb-…` id (prefix ok).
fn resolve_inbox_target(
    path: &std::path::Path,
    arg: Option<&String>,
) -> Result<theway_transport::inbox::InboxEntry, String> {
    let Some(arg) = arg else {
        return Err("usage: /inbox claim|dismiss <n or inb-id>".into());
    };
    let entries = theway_transport::inbox::list_new(path).map_err(|e| format!("inbox: {e}"))?;
    if let Ok(n) = arg.parse::<usize>() {
        return entries
            .get(n.saturating_sub(1))
            .cloned()
            .ok_or_else(|| format!("no inbox entry #{n} (have {})", entries.len()));
    }
    entries
        .iter()
        .find(|entry| entry.id.starts_with(arg.as_str()))
        .cloned()
        .ok_or_else(|| format!("no new inbox entry matching '{arg}'"))
}

fn set_dynamic_trigger_enabled(target: Option<&String>, enabled: bool) -> CommandOutcome {
    let Some(id) = target else {
        let action = if enabled { "enable" } else { "disable" };
        return CommandOutcome::Error(format!("usage: /triggers {action} <id>"));
    };
    match crate::triggers::global_registry().set_rule_enabled(id, enabled) {
        Ok(Some(rule)) => {
            let state = if rule.enabled { "enabled" } else { "disabled" };
            cprintln!("{state} trigger {}", rule.id);
            cprintln!("  condition: {}", rule.condition);
            cprintln!("  action: {}", rule.action);
            if rule.enabled && rule.fire_once {
                cprintln!("  fire_once: true (will disable again after the next successful match)");
            }
            CommandOutcome::Handled
        }
        Ok(None) => CommandOutcome::Error(format!("no dynamic trigger rule with id '{id}'")),
        Err(e) => CommandOutcome::Error(e.to_string()),
    }
}
