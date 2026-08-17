//! Tests for `system_prompt` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn compose_system_prompt_puts_base_then_cwd_then_memory() {
    // Arrange
    let cwd = std::path::Path::new("/tmp/waypoint");
    let tools = vec!["bash".to_string(), "grep".to_string()];

    // Act
    let prompt = compose_system_prompt(cwd, "Remember: be concise.", &tools);

    // Assert
    let base = prompt.find("You are theway").expect("base prompt present");
    let tools_at = prompt.find("bash, grep").expect("tool inventory present");
    let cwd_at = prompt
        .find("Current working directory: /tmp/waypoint")
        .expect("cwd present");
    let memory_at = prompt
        .find("Remember: be concise.")
        .expect("memory present");
    assert!(base < tools_at && tools_at < cwd_at && cwd_at < memory_at);
}

#[test]
fn compose_system_prompt_with_empty_memory_omits_memory_block() {
    // Arrange
    let cwd = std::path::Path::new("/tmp/waypoint");

    // Act
    let prompt = compose_system_prompt(cwd, "", &[]);

    // Assert
    assert!(prompt.contains("Current working directory: /tmp/waypoint\n"));
    assert!(!prompt.contains("Remember"));
}

#[test]
fn render_base_prompt_uses_no_tools_registered_for_empty_inventory() {
    // Act
    let prompt = render_base_prompt(&[]);

    // Assert
    assert!(prompt.contains("no tools registered"));
}

#[test]
fn render_base_prompt_joins_tool_names_in_inventory() {
    // Act
    let prompt = render_base_prompt(&["bash".to_string(), "grep".to_string()]);

    // Assert
    assert!(prompt.contains("bash, grep"));
}
