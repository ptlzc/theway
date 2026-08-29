//! System prompt composition for the startup harness.

pub fn compose_system_prompt(
    cwd: &std::path::Path,
    memory: &str,
    tool_names: &[String],
    lineage: Option<&str>,
    harness_intro: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str(&render_base_prompt(tool_names, harness_intro));
    s.push_str("\n\n<environment>\n");
    s.push_str(&format!("Current working directory: {}\n", cwd.display()));
    if !memory.is_empty() {
        s.push_str("<memory>\n");
        s.push_str(memory);
        s.push_str("\n</memory>\n");
    }
    s.push_str("</environment>");
    if let Some(lineage) = lineage.filter(|lineage| !lineage.trim().is_empty()) {
        s.push_str("\n\n<lineage>\n");
        s.push_str(lineage);
        s.push_str("\n</lineage>");
    }
    s
}

/// Build the `<harness>` and `<tools>` blocks. The harness block explains the runtime model
/// before the tool inventory so the model understands session storage, exploration, and
/// graph/subagent orchestration before seeing which tools implement them. The tool inventory
/// is rendered from the actual registered tool definitions, grouped by category.
fn render_base_prompt(tool_names: &[String], harness_intro: Option<&str>) -> String {
    let intro = harness_intro
        .unwrap_or("You are theway, a minimal coding assistant running in a terminal.");
    let inventory = render_tool_inventory(tool_names);
    let template = r#"
<harness>
{intro}

Session model: the conversation is stored as append-only session entries (messages, custom facts, compaction and branch summaries). Compaction replaces older entries with a summary plus a recent tail; collapsed sessions become session graph nodes and their raw transcript stays available through session_graph_read.

Exploration: read files before editing; use outline for large-file structure, then read with offset/limit; use grep for repository search. Tool outputs larger than the context budget are virtualized and can be paged back on demand.

Graph and subagent orchestration: dag_* tools plan and monitor dependent subagent runs; subagents run in isolated contexts and return summaries; session_graph_* tools inspect or take over collapsed session graphs. Use todo_write for linear plans and dag_plan for 2+ dependent subtasks.

Behavioral rules:
Prefer running a tool over guessing. When making file changes, read the file first to confirm the exact current contents, then edit or write. Keep responses concise.
When the user asks for a fixed time, recurring, scheduled, hourly, daily, weekly, crontab, 定时任务, 每小时, or similar time-based job, call new_cron_job instead of new_trigger.
When the user asks to view, list, show, inspect, or find scheduled jobs or cron job ids, call list_cron_jobs.
When the user asks to pause or disable a scheduled job or cron job, call set_cron_job_state with enabled=false; enabling/resuming should point the user to /cron enable <id> until confirmation support is wired.
When the user asks to delete, remove, or clear scheduled jobs or cron jobs, call remove_cron_job first with confirm=false to preview, then only call confirm=true after explicit user confirmation.
When the user asks to create a trigger, reminder, watcher, or automation, call new_trigger and extract a natural-language condition and action from their request. Dynamic triggers fire once by default; set fire_once=false only when the user explicitly asks for a repeating trigger. Trigger output is shown in the TUI and audit by default; set promote_to_chat=true only when the user explicitly asks for trigger results to enter the main chat context or be visible to future turns.
When the user asks to view, list, show, inspect, or find trigger ids, call list_triggers.
When the user asks to pause, disable, enable, or resume a dynamic trigger, call set_trigger_state.
When the user asks to delete, remove, or clear dynamic triggers, call remove_trigger.
When the user asks to create, save, or codify a reusable skill, workflow, checklist, or convention, or to summarize recent work or this conversation into a skill (技能, 保存为技能, 把刚才的工作总结成 skill), call skill_builder with structured name/description/instructions. For summarize-into-skill requests, distill the generalizable steps from the conversation — what was actually done, the commands used, the pitfalls — not a transcript. Call once without confirm to preview and show the user the planned name and description, then call with confirm=true after they agree. Use install_skill only for installing an existing SKILL.md from a URL, file, or pasted content.
</harness>

<tools>
{inventory}
</tools>
"#;
    let rendered = template
        .replace("{intro}", intro)
        .replace("{inventory}", &inventory);
    dedent(rendered.trim_start_matches('\n').trim_end())
}

fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let common_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.len() > common_indent {
            out.push_str(&line[common_indent..]);
        } else {
            out.push_str(line.trim_end());
        }
    }
    out
}

fn render_tool_inventory(tool_names: &[String]) -> String {
    if tool_names.is_empty() {
        return "no tools registered".to_string();
    }

    let mut groups = Vec::<(String, Vec<String>)>::new();
    for name in tool_names {
        let category = tool_category(name).to_string();
        if let Some((_, names)) = groups.iter_mut().find(|(label, _)| label == &category) {
            names.push(name.clone());
        } else {
            groups.push((category, vec![name.clone()]));
        }
    }

    groups
        .into_iter()
        .map(|(category, mut names)| {
            names.sort();
            names.dedup();
            format!("- {category}: {}", names.join(", "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_category(name: &str) -> &'static str {
    if name.starts_with("dag_")
        || name.starts_with("subagent")
        || name.starts_with("agent_profile_")
        || name == "todo_write"
        || name == "ask_user"
    {
        "Orchestration & planning"
    } else if name.starts_with("session_graph_") {
        "Session graph"
    } else if name.starts_with("memory_")
        || name.starts_with("enhanced_")
        || name == "grep"
        || name == "outline"
        || name == "read_output"
        || name == "find_file_by_name"
        || name.starts_with("crg__")
    {
        "Context & search"
    } else if matches!(name, "read" | "write" | "edit" | "ls" | "find" | "git") {
        "Files"
    } else if name.starts_with("web_") {
        "Web"
    } else if matches!(
        name,
        "bash" | "exec" | "get_output" | "kill_shell" | "write_to_process"
    ) {
        "Execution"
    } else if name.contains("cron")
        || name.contains("trigger")
        || name.starts_with("skill")
        || name.starts_with("install_skill")
        || name.starts_with("remove_skill")
        || name.starts_with("set_skill_state")
    {
        "Automation & skills"
    } else {
        "Other"
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("context/system_prompt");
