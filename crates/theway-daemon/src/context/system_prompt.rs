//! System prompt composition for the startup harness.

pub fn compose_system_prompt(
    cwd: &std::path::Path,
    memory: &str,
    tool_names: &[String],
    lineage: Option<&str>,
    harness_intro: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str(&render_tools_block(tool_names));
    s.push_str("\n\n");
    s.push_str(&render_harness_block(harness_intro));
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

/// Render the `<tools>` block first: the model learns which capabilities are
/// attached before reading any harness prose.
fn render_tools_block(tool_names: &[String]) -> String {
    format!("<tools>\n{}\n</tools>", render_tool_inventory(tool_names))
}

/// Render the `<harness>` block. It explains the runtime model after the tool
/// inventory so the model can map session storage, exploration, collapse, and
/// graph/subagent orchestration concepts onto the tools it just saw.
fn render_harness_block(harness_intro: Option<&str>) -> String {
    let intro = harness_intro
        .unwrap_or("You are theway, a minimal coding assistant running in a terminal.");
    let template = r#"
<harness>
{intro}

Session model: the conversation is stored as an append-only message tree. Entries are messages, custom facts, compaction entries, branch summaries, labels, and session info. Compaction keeps the newest tail verbatim and replaces older entries with a compaction summary; branch summaries record what a fork/branch inherits. When the context budget is tight, oversized tool results are virtualized into compact placeholders — read the full stored output on demand with session_tool_result or search it with session_tool_result_grep. Never treat a virtualized placeholder as the complete tool output.

Collapse model: /collapse turns the current session into a session graph node and starts a child session. The child context carries only the compact summary (collapse_context) plus lineage identity; the full old transcript stays in the old session. Read it on demand with session_graph_read (paginated raw transcript), inspect live graphs with session_graph_status, wait for terminal state with session_graph_wait, list nodes with session_graph_list, and take over graphs with session_graph_attach or /collapse --adopt. Never assume the compact summary contains every detail from the collapsed session.

Exploration: read files before editing; use outline for large-file structure, then read with offset/limit; use grep for repository search. Tool outputs larger than the context budget are virtualized and can be paged back on demand via the session_tool_result tools.

Graph and subagent orchestration principles: use todo_write for linear work and dag_plan for 2+ dependent subtasks; every non-root DAG node must declare its dependency; keep parallel nodes file-disjoint and pin each node's task text to the exact directory and files it may touch; harvest DAG results only with dag_wait (never subagent_wait for DAG nodes); do not manually launch background jobs for DAG nodes; inspect failures with dag_inspect and recover with dag_retry/dag_skip; verify final results against the latest worktree state; only the orchestrator writes git history. Subagents run in isolated contexts and return summaries; their full output is stored and can be read back rather than polluting the parent context.

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
"#;
    let rendered = template.replace("{intro}", intro);
    dedent(rendered.trim_start_matches('\n').trim_end())
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

#[cfg(test)]
tests_bridge_macro::tests_bridge!("context/system_prompt");
