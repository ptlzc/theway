//! Tests for `system_prompt` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn compose_system_prompt_puts_harness_then_tools_then_cwd_then_memory() {
    // Arrange
    let cwd = std::path::Path::new("/tmp/waypoint");
    let tools = vec!["bash".to_string(), "enhanced_grep".to_string()];

    // Act
    let prompt = compose_system_prompt(cwd, "Remember: be concise.", &tools, None, None);

    // Assert
    let harness_at = prompt.find("<harness>").expect("harness block present");
    let base = prompt.find("You are theway").expect("base prompt present");
    let tools_at = prompt.find("<tools>").expect("tools block present");
    let inventory_at = prompt.find("Execution: bash").expect("tool inventory present");
    let cwd_at = prompt
        .find("Current working directory: /tmp/waypoint")
        .expect("cwd present");
    let memory_at = prompt
        .find("Remember: be concise.")
        .expect("memory present");
    assert!(harness_at < base);
    assert!(base < tools_at);
    assert!(tools_at < inventory_at);
    assert!(inventory_at < cwd_at && cwd_at < memory_at);
    assert!(prompt.contains("</tools>"));
    assert!(prompt.contains("<environment>"));
}

#[test]
fn compose_system_prompt_with_empty_memory_omits_memory_block() {
    // Arrange
    let cwd = std::path::Path::new("/tmp/waypoint");

    // Act
    let prompt = compose_system_prompt(cwd, "", &[], None, None);

    // Assert
    assert!(prompt.contains("Current working directory: /tmp/waypoint\n"));
    assert!(!prompt.contains("Remember"));
}

#[test]
fn compose_system_prompt_appends_lineage_block_when_provided() {
    let cwd = std::path::Path::new("/tmp/waypoint");
    let lineage = "## Session lineage\n\nThis session continues from old-session.\nPrevious context summary: explored X.\nUse session_graph_read to inspect the old graph.";

    let prompt = compose_system_prompt(
        cwd,
        "",
        &["session_graph_read".to_string()],
        Some(lineage),
        None,
    );

    assert!(prompt.contains("<lineage>"));
    assert!(prompt.contains("## Session lineage"));
    assert!(prompt.contains("old-session"));
    assert!(prompt.contains("session_graph_read"));
}

#[test]
fn compose_system_prompt_omits_lineage_when_none() {
    let cwd = std::path::Path::new("/tmp/waypoint");

    let prompt = compose_system_prompt(cwd, "", &[], None, None);

    assert!(!prompt.contains("Session lineage"));
}

#[test]
fn render_tools_block_uses_no_tools_registered_for_empty_inventory() {
    // Act
    let prompt = render_tools_block(&[]);

    // Assert
    assert!(prompt.contains("<tools>"));
    assert!(prompt.contains("no tools registered"));
}

#[test]
fn render_tools_block_groups_tool_names_by_category() {
    // Act
    let prompt = render_tools_block(&[
        "bash".to_string(),
        "enhanced_grep".to_string(),
        "dag_plan".to_string(),
        "session_graph_read".to_string(),
    ]);

    // Assert
    assert!(prompt.contains("- Execution: bash"));
    assert!(prompt.contains("- Context & search: enhanced_grep"));
    assert!(prompt.contains("- Orchestration & planning: dag_plan"));
    assert!(prompt.contains("- Session graph: session_graph_read"));
}

#[test]
fn render_harness_block_describes_runtime_model() {
    // Act
    let prompt = render_harness_block(None);

    // Assert
    assert!(prompt.contains("<harness>"));
    assert!(
        prompt.contains("Session model: the conversation is stored as an append-only message tree")
    );
    assert!(prompt.contains("session_tool_result_grep"));
    assert!(prompt.contains(
        "Collapse model: /collapse turns the current session into a session graph node"
    ));
    assert!(prompt.contains("bounded rolling compact summary"));
    assert!(prompt.contains("same fixed components"));
    assert!(prompt.contains("goal, completed work, key decisions, next steps, critical context"));
    assert!(prompt.contains("lineage block only records the collapse event"));
    assert!(prompt.contains("session_graph_read"));
    assert!(prompt.contains("Exploration: read files before editing"));
    assert!(prompt.contains("Graph and subagent orchestration principles"));
    assert!(prompt.contains("harvest DAG results only with dag_wait"));
}

#[test]
fn compose_system_prompt_filters_blank_lineage() {
    let cwd = std::path::Path::new("/tmp/waypoint");

    let prompt = compose_system_prompt(cwd, "", &[], Some("   \n\t "), None);

    assert!(!prompt.contains("<lineage>"));
    assert!(!prompt.contains("Session lineage"));
}

#[test]
fn tool_category_covers_all_remaining_groups() {
    assert_eq!(tool_category("read"), "Files");
    assert_eq!(tool_category("write"), "Files");
    assert_eq!(tool_category("web_fetch"), "Web");
    assert_eq!(tool_category("list_cron_jobs"), "Automation & skills");
    assert_eq!(tool_category("skill_builder"), "Automation & skills");
    assert_eq!(tool_category("totally-unknown-tool"), "Other");
    assert_eq!(tool_category("subagent_wait"), "Orchestration & planning");
    assert_eq!(tool_category("session_graph_wait"), "Session graph");
}

#[test]
fn render_tool_inventory_deduplicates_and_sorts_within_category() {
    let inventory = render_tool_inventory(&[
        "bash".to_string(),
        "get_output".to_string(),
        "bash".to_string(),
        "enhanced_grep".to_string(),
        "enhanced_grep".to_string(),
    ]);
    assert_eq!(
        inventory,
        "- Execution: bash, get_output\n- Context & search: enhanced_grep"
    );
}

#[test]
fn render_tool_inventory_includes_unfiled_and_automation_tools() {
    let inventory = render_tool_inventory(&[
        "web_search".to_string(),
        "install_skill".to_string(),
        "odd_tool".to_string(),
    ]);
    assert!(inventory.contains("- Web: web_search"));
    assert!(inventory.contains("- Automation & skills: install_skill"));
    assert!(inventory.contains("- Other: odd_tool"));
}

#[test]
fn dedent_handles_zero_indent_and_blank_lines() {
    assert_eq!(dedent("no indent\n\nsecond"), "no indent\n\nsecond");
    assert_eq!(dedent("    indented\n      deeper"), "indented\n  deeper");
    assert_eq!(dedent(""), "");
}

#[test]
fn render_harness_block_uses_custom_harness_intro() {
    // Act
    let prompt = render_harness_block(Some("You are a database migration specialist."));

    // Assert
    assert!(prompt.contains("You are a database migration specialist."));
    assert!(!prompt.contains("minimal coding assistant"));
}
