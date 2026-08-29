//! Tests for `system_prompt` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn compose_system_prompt_puts_base_then_cwd_then_memory() {
    // Arrange
    let cwd = std::path::Path::new("/tmp/waypoint");
    let tools = vec!["bash".to_string(), "enhanced_grep".to_string()];

    // Act
    let prompt = compose_system_prompt(cwd, "Remember: be concise.", &tools, None, None);

    // Assert
    let base = prompt.find("You are theway").expect("base prompt present");
    let tools_at = prompt.find("Execution: bash").expect("tool inventory present");
    let cwd_at = prompt
        .find("Current working directory: /tmp/waypoint")
        .expect("cwd present");
    let memory_at = prompt
        .find("Remember: be concise.")
        .expect("memory present");
    assert!(base < tools_at && tools_at < cwd_at && cwd_at < memory_at);
    assert!(prompt.find("<harness>") < Some(tools_at));
    assert!(tools_at < prompt.find("</tools>").unwrap());
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
fn render_base_prompt_uses_no_tools_registered_for_empty_inventory() {
    // Act
    let prompt = render_base_prompt(&[], None);

    // Assert
    assert!(prompt.contains("no tools registered"));
}

#[test]
fn render_base_prompt_groups_tool_names_by_category() {
    // Act
    let prompt = render_base_prompt(
        &[
            "bash".to_string(),
            "enhanced_grep".to_string(),
            "dag_plan".to_string(),
            "session_graph_read".to_string(),
        ],
        None,
    );

    // Assert
    assert!(prompt.contains("- Execution: bash"));
    assert!(prompt.contains("- Context & search: enhanced_grep"));
    assert!(prompt.contains("- Orchestration & planning: dag_plan"));
    assert!(prompt.contains("- Session graph: session_graph_read"));
}

#[test]
fn render_base_prompt_describes_runtime_model_before_tools() {
    // Act
    let prompt = render_base_prompt(&["dag_plan".to_string()], None);

    // Assert
    assert!(prompt.contains("<harness>"));
    assert!(prompt.contains("Session model: the conversation is stored as an append-only message tree"));
    assert!(prompt.contains("session_tool_result_grep"));
    assert!(prompt.contains("Collapse model: /collapse turns the current session into a session graph node"));
    assert!(prompt.contains("session_graph_read"));
    assert!(prompt.contains("Exploration: read files before editing"));
    assert!(prompt.contains("Graph and subagent orchestration principles"));
    assert!(prompt.contains("harvest DAG results only with dag_wait"));
    assert!(prompt.contains("<tools>"));
    assert!(prompt.find("<tools>").unwrap() > prompt.find("</harness>").unwrap());
}

#[test]
fn render_base_prompt_uses_custom_harness_intro() {
    // Act
    let prompt = render_base_prompt(&[], Some("You are a database migration specialist."));

    // Assert
    assert!(prompt.contains("You are a database migration specialist."));
    assert!(!prompt.contains("minimal coding assistant"));
}
