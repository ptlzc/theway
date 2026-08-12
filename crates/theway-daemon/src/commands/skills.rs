//! `/skills` — list, install, inspect, reload, enable, disable, and remove skills.

use super::*;

use theway::commands::CommandCtx;

pub struct SkillsCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for SkillsCommand {
    fn name(&self) -> &'static str {
        "skills"
    }
    fn description(&self) -> &'static str {
        "list, install, inspect, reload, enable, disable, or remove skills"
    }
    fn usage(&self) -> &'static str {
        "[install [--confirm] [--overwrite] <url|path>|show <name>|reload|enable <name> [source]|disable <name> [source]|remove [--confirm] <name> [source]]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        match argv.first().map(String::as_str) {
            None | Some("list" | "ls") => {
                print_skills_list(&ctx.harness.skills());
                CommandOutcome::Handled
            }
            Some("install") => install_skill(&argv[1..], ctx).await,
            Some("show") => show_skill(&argv[1..], ctx),
            Some("reload") => reload_skills(ctx).await,
            Some("enable") => set_skill_enabled(&argv[1..], ctx, true).await,
            Some("disable") => set_skill_enabled(&argv[1..], ctx, false).await,
            Some("remove") => remove_skill(&argv[1..], ctx).await,
            Some(_) => CommandOutcome::Error(
                "usage: /skills [install [--confirm] [--overwrite] <url|path>|show <name>|reload|enable <name> [source]|disable <name> [source]|remove [--confirm] <name> [source]]"
                    .into(),
            ),
        }
    }
}

fn print_skills_list(skills: &[Skill]) {
    if skills.is_empty() {
        cprintln!(
            "(no skills loaded — drop SKILL.md files under ~/.theway/skills/<name>/ or <cwd>/.theway/skills/<name>/)"
        );
    } else {
        cprintln!("Loaded skills ({}):", skills.len());
        for s in skills {
            let disabled = if s.disable_model_invocation {
                "  [disabled: disable_model_invocation=true]"
            } else {
                ""
            };
            cprintln!("  - {}  ({}){}", s.name, s.source.label(), disabled);
            if !s.description.is_empty() {
                cprintln!("      {}", s.description);
            }
            cprintln!("      path: {}", s.file_path);
        }
    }
}

fn show_skill(argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
    let Some(name) = argv.first() else {
        return CommandOutcome::Error("usage: /skills show <name> [source]".into());
    };
    let source = match optional_skill_source(argv.get(1)) {
        Ok(source) => source,
        Err(e) => return CommandOutcome::Error(e),
    };
    let skills = ctx.harness.skills();
    let skill = match resolve_active_skill(&skills, name, source) {
        Ok(skill) => skill,
        Err(e) => return CommandOutcome::Error(e),
    };
    cprintln!("Skill: {} ({})", skill.name, skill.source.label());
    cprintln!(
        "Status: {}",
        if skill.disable_model_invocation {
            "disabled"
        } else {
            "enabled"
        }
    );
    if !skill.description.is_empty() {
        cprintln!("Description: {}", skill.description);
    }
    cprintln!("Path: {}", skill.file_path);
    cprintln!("Body: not shown; use the file path if you need to inspect the full skill.");
    CommandOutcome::Handled
}

async fn reload_skills(ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
    match ctx.harness.reload_skills_from_disk().await {
        Ok(out) => {
            cprintln!(
                "reloaded skills: {} loaded, {} diagnostics",
                out.skills.len(),
                out.diagnostics.len()
            );
            CommandOutcome::Handled
        }
        Err(e) => CommandOutcome::Error(format!("reload skills failed: {e}")),
    }
}

async fn install_skill(argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
    let parsed = match parse_skill_install_args(argv) {
        Ok(parsed) => parsed,
        Err(e) => return CommandOutcome::Error(e),
    };
    let source = skill_install_source(parsed.target, ctx.cwd);
    let params = json!({
        "source": source,
        "confirm": parsed.confirm,
        "overwrite": parsed.overwrite,
    });
    let cell = skill_harness_cell(ctx);
    let tool = theway_core::tools::install_skill::InstallSkillTool::new(cell);
    match tool
        .execute(
            "slash-skills-install",
            params,
            CancellationToken::new(),
            None,
        )
        .await
    {
        Ok(result) => {
            print_install_skill_result(&result, &parsed);
            CommandOutcome::Handled
        }
        Err(e) => CommandOutcome::Error(format!("install skill failed: {e}")),
    }
}

struct InstallSkillArgs<'a> {
    target: &'a str,
    confirm: bool,
    overwrite: bool,
}

fn parse_skill_install_args(argv: &[String]) -> Result<InstallSkillArgs<'_>, String> {
    let mut confirm = false;
    let mut overwrite = false;
    let mut positional = Vec::new();
    for arg in argv {
        match arg.as_str() {
            "--confirm" | "--yes" => confirm = true,
            "--overwrite" => overwrite = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown option for /skills install: {other}"));
            }
            _ => positional.push(arg.as_str()),
        }
    }
    match positional.as_slice() {
        [target] => Ok(InstallSkillArgs {
            target,
            confirm,
            overwrite,
        }),
        _ => Err("usage: /skills install [--confirm] [--overwrite] <https-url|path>".into()),
    }
}

fn skill_install_source(target: &str, cwd: &std::path::Path) -> serde_json::Value {
    if target.starts_with("http://") || target.starts_with("https://") {
        json!({ "type": "url", "url": target })
    } else {
        let path = std::path::PathBuf::from(target);
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        json!({ "type": "path", "path": path.to_string_lossy().to_string() })
    }
}

fn print_install_skill_result(result: &theway_core::AgentToolResult, args: &InstallSkillArgs) {
    let phase = result.details.get("phase").and_then(|v| v.as_str());
    if phase == Some("preview") {
        let name = result
            .details
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let target = result
            .details
            .get("target_path")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let size = result
            .details
            .get("size")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let existing = result
            .details
            .get("existing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let overwrite_required = result
            .details
            .get("overwrite_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        cprintln!(
            "skill install preview: {name} -> {target} ({size}B, existing={existing}, overwrite_required={overwrite_required})"
        );
        let overwrite = if overwrite_required && !args.overwrite {
            " --overwrite"
        } else {
            ""
        };
        cprintln!("run `/skills install --confirm{overwrite} <same-url-or-path>` to install");
        return;
    }
    for line in tool_result_text(result).lines() {
        cprintln!("{line}");
    }
}

async fn set_skill_enabled(
    argv: &[String],
    ctx: &CommandCtx<'_, DaemonCtx>,
    enabled: bool,
) -> CommandOutcome {
    let Some(name) = argv.first() else {
        let verb = if enabled { "enable" } else { "disable" };
        return CommandOutcome::Error(format!("usage: /skills {verb} <name> [source]"));
    };
    let source = match optional_skill_source(argv.get(1)) {
        Ok(source) => source,
        Err(e) => return CommandOutcome::Error(e),
    };
    let skills = ctx.harness.skills();
    let skill = match resolve_active_skill(&skills, name, source) {
        Ok(skill) => skill,
        Err(e) => return CommandOutcome::Error(e),
    };
    let source = skill.source;
    let name = skill.name.clone();
    let was_enabled = !skill.disable_model_invocation;

    if was_enabled == enabled {
        cprintln!(
            "skill already {}: {} ({})",
            if enabled { "enabled" } else { "disabled" },
            name,
            source.label()
        );
        return CommandOutcome::Handled;
    }

    if let Err(e) =
        crate::skill_overrides::set_and_save(&theway::config::base_dir(), &name, source, enabled)
            .await
    {
        return CommandOutcome::Error(format!("persist skill state failed: {e}"));
    }
    match ctx.harness.reload_skills_from_disk().await {
        Ok(out) => {
            write_skill_state_audit(ctx, &name, source, was_enabled, enabled).await;
            let diagnostics = if out.diagnostics.is_empty() {
                String::new()
            } else {
                format!(" ({} diagnostics)", out.diagnostics.len())
            };
            cprintln!(
                "{} skill: {} ({}){}",
                if enabled { "enabled" } else { "disabled" },
                name,
                source.label(),
                diagnostics
            );
            CommandOutcome::Handled
        }
        Err(e) => CommandOutcome::Error(format!("reload after skill state change failed: {e}")),
    }
}

async fn remove_skill(argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
    let parsed = match parse_skill_remove_args(argv) {
        Ok(parsed) => parsed,
        Err(e) => return CommandOutcome::Error(e),
    };
    let mut params = json!({
        "name": parsed.name,
        "confirm": parsed.confirm,
    });
    if let Some(source) = parsed.source {
        params["source"] = json!(source.label());
    }
    let cell = skill_harness_cell(ctx);
    let tool = theway_core::tools::remove_skill::RemoveSkillTool::new(cell);
    match tool
        .execute(
            "slash-skills-remove",
            params,
            CancellationToken::new(),
            None,
        )
        .await
    {
        Ok(result) => {
            print_remove_skill_result(&result);
            CommandOutcome::Handled
        }
        Err(e) => CommandOutcome::Error(format!("remove skill failed: {e}")),
    }
}

struct RemoveSkillArgs<'a> {
    name: &'a str,
    source: Option<SkillSource>,
    confirm: bool,
}

fn parse_skill_remove_args(argv: &[String]) -> Result<RemoveSkillArgs<'_>, String> {
    let mut confirm = false;
    let mut positional = Vec::new();
    for arg in argv {
        match arg.as_str() {
            "--confirm" | "--yes" => confirm = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown option for /skills remove: {other}"));
            }
            _ => positional.push(arg.as_str()),
        }
    }
    match positional.as_slice() {
        [name] => Ok(RemoveSkillArgs {
            name,
            source: None,
            confirm,
        }),
        [name, source] => Ok(RemoveSkillArgs {
            name,
            source: Some(parse_skill_source(source)?),
            confirm,
        }),
        _ => Err("usage: /skills remove [--confirm] <name> [source]".into()),
    }
}

fn print_remove_skill_result(result: &theway_core::AgentToolResult) {
    let phase = result.details.get("phase").and_then(|v| v.as_str());
    if phase == Some("preview") {
        let name = result
            .details
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let target = result
            .details
            .get("target_path")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        cprintln!("skill remove preview: {name} (user) -> {target}");
        cprintln!("run `/skills remove --confirm {name}` to remove it");
        return;
    }
    for line in tool_result_text(result).lines() {
        cprintln!("{line}");
    }
}

fn skill_harness_cell(
    ctx: &CommandCtx<'_, DaemonCtx>,
) -> theway_core::tools::skill::SkillHarnessCell {
    let cell = std::sync::Arc::new(once_cell::sync::OnceCell::new());
    // This is a fresh cell scoped to a single slash command invocation, so set() can only fail
    // if this helper is called incorrectly inside the same invocation.
    let _ = cell.set(ctx.harness.clone());
    cell
}

fn tool_result_text(result: &theway_core::AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            UserContentBlock::Text(text) => Some(text.text.as_str()),
            UserContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn write_skill_state_audit(
    ctx: &CommandCtx<'_, DaemonCtx>,
    name: &str,
    source: SkillSource,
    before_enabled: bool,
    after_enabled: bool,
) {
    let audit = json!({
        "op": "set_state",
        "actor": "slash",
        "name": name,
        "source": source.label(),
        "before_enabled": before_enabled,
        "after_enabled": after_enabled,
    });
    if let Err(e) = ctx
        .harness
        .session()
        .append_custom("skill_control_plane", Some(audit))
        .await
    {
        tracing::warn!(
            skill = %name,
            error = %e,
            "skill_control_plane audit write failed; slash state change itself succeeded"
        );
    }
}

fn optional_skill_source(raw: Option<&String>) -> Result<Option<SkillSource>, String> {
    raw.map(|s| parse_skill_source(s).map(Some))
        .unwrap_or(Ok(None))
}

pub(super) fn parse_skill_source(raw: &str) -> Result<SkillSource, String> {
    match raw {
        "builtin" => Ok(SkillSource::Builtin),
        "user" => Ok(SkillSource::User),
        "project" => Ok(SkillSource::Project),
        _ => Err("invalid skill source; expected one of: builtin, user, project".into()),
    }
}

fn resolve_active_skill<'a>(
    skills: &'a [Skill],
    name: &str,
    source: Option<SkillSource>,
) -> Result<&'a Skill, String> {
    let matches = skills
        .iter()
        .filter(|skill| skill.name == name && source.map(|s| skill.source == s).unwrap_or(true))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [skill] => Ok(*skill),
        [] => {
            let source_hint = source
                .map(|source| format!(" {} ", source.label()))
                .unwrap_or_else(|| " ".into());
            Err(format!(
                "no active{source_hint}skill named '{name}'. Run /skills to list loaded skills."
            ))
        }
        _ => Err(format!(
            "multiple active skills named '{name}'; pass source: builtin, user, or project"
        )),
    }
}
