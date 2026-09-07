//! Tests for `ui_state.rs`: the TUI display-switch persistence file.

use super::*;

#[test]
fn parse_roundtrips_all_fields() {
    let state = parse(
        r#"
[feed]
thinking_mode = "peek"

[panel]
mode = "shown"
position = "left"
show_hooks = true
show_runtime = true
"#,
    );
    assert_eq!(state.thinking_mode, Some(crate::feed_render::ThinkingMode::Peek));
    assert_eq!(state.panel_mode, Some(crate::ui::SidePanelMode::Shown(crate::ui::TRIGGER_PANEL_WIDTH)));
    assert_eq!(state.panel_position, Some(crate::ui::SidePanelPosition::Left));
    assert!(state.show_hooks);
    assert!(state.show_runtime);
}

#[test]
fn parse_show_flags_default_to_hidden() {
    // Absent or malformed flags fall back to hidden.
    let state = parse("[panel]\nmode = \"shown\"\nshow_hooks = \"yes\"\n");
    assert!(!state.show_hooks);
    assert!(!state.show_runtime);
    let state = parse("[panel]\nshow_hooks = true\n");
    assert!(state.show_hooks);
    assert!(!state.show_runtime);
    let namespaced = parse_namespaced("[ui.panel]\nshow_runtime = true\n");
    assert!(!namespaced.show_hooks);
    assert!(namespaced.show_runtime);
}

#[test]
fn parse_unknown_values_fall_back_to_none() {
    let state = parse(
        r#"
[feed]
thinking_mode = "banana"

[panel]
mode = "wide"
position = "diagonal"

[graph]
position = "behind-the-fridge"
"#,
    );
    assert_eq!(state, UiState::default());
}

#[test]
fn parse_graph_position() {
    let state = parse("[graph]\nposition = \"side-panel\"");
    assert_eq!(
        state.graph_position,
        Some(crate::ui::GraphPosition::SidePanel)
    );
    let state = parse("[graph]\nposition = \"composer-top\"");
    assert_eq!(
        state.graph_position,
        Some(crate::ui::GraphPosition::ComposerTop)
    );
}

#[test]
fn parse_missing_file_is_default() {
    let state = parse("");
    assert_eq!(state, UiState::default());
}

#[test]
fn namespaced_ui_fields_parse_same_as_top_level() {
    let namespaced = parse_namespaced(
        r#"
[ui.feed]
thinking_mode = "peek"

[ui.panel]
mode = "shown"
position = "left"

[ui.graph]
position = "side-panel"
"#,
    );
    let top_level = parse(
        r#"
[feed]
thinking_mode = "peek"

[panel]
mode = "shown"
position = "left"

[graph]
position = "side-panel"
"#,
    );
    assert_eq!(namespaced, top_level);
    assert_eq!(
        namespaced.thinking_mode,
        Some(crate::feed_render::ThinkingMode::Peek)
    );
    assert_eq!(
        namespaced.panel_mode,
        Some(crate::ui::SidePanelMode::Shown(crate::ui::TRIGGER_PANEL_WIDTH))
    );
    assert_eq!(
        namespaced.panel_position,
        Some(crate::ui::SidePanelPosition::Left)
    );
    assert_eq!(namespaced.graph_position, Some(crate::ui::GraphPosition::SidePanel));
}

#[test]
fn load_from_config_text_parses_ui_section_in_full_config() {
    let state = load_from_config_text(
        r#"
[model]
provider = "openai"
name = "gpt-4o"

[theme]
name = "dracula"

[[server]]
name = "local"
addr = "unix:///tmp/theway"

[ui.feed]
thinking_mode = "hidden"

[ui.panel]
mode = "hidden"
position = "bottom"

[ui.graph]
position = "composer-top"
"#,
    );
    assert_eq!(
        state.thinking_mode,
        Some(crate::feed_render::ThinkingMode::Hidden)
    );
    assert_eq!(state.panel_mode, Some(crate::ui::SidePanelMode::Hidden));
    assert_eq!(
        state.panel_position,
        Some(crate::ui::SidePanelPosition::Bottom)
    );
    assert_eq!(
        state.graph_position,
        Some(crate::ui::GraphPosition::ComposerTop)
    );
}

#[test]
fn parse_namespaced_without_ui_table_falls_back_to_top_level() {
    let text =
        "[feed]\nthinking_mode = \"full\"\n[panel]\nposition = \"right\"\n";
    let namespaced = parse_namespaced(text);
    assert_eq!(namespaced, parse(text));
    assert_eq!(
        namespaced.thinking_mode,
        Some(crate::feed_render::ThinkingMode::Full)
    );
    assert_eq!(
        namespaced.panel_position,
        Some(crate::ui::SidePanelPosition::Right)
    );
}

#[test]
fn render_emits_only_set_fields() {
    let state = UiState {
        thinking_mode: Some(crate::feed_render::ThinkingMode::Hidden),
        panel_mode: Some(crate::ui::SidePanelMode::Hidden),
        panel_position: None,
        graph_position: None,
        show_hooks: true,
        show_runtime: false,
    };
    let text = render(&state);
    assert!(text.contains("[feed]\nthinking_mode = \"hidden\""));
    assert!(text.contains("[panel]\nmode = \"hidden\""));
    assert!(text.contains("show_hooks = true"));
    assert!(!text.contains("show_runtime"));
    assert!(!text.contains("position"));
    assert!(!text.contains("[graph]"));
    // Rendered output parses back to the same state.
    assert_eq!(parse(&text), state);
}

#[test]
fn save_and_load_roundtrip() {
    let dir = std::env::temp_dir().join(format!("theway-ui-state-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui-state.toml");
    let _ = std::fs::remove_file(&path);

    let state = UiState {
        thinking_mode: Some(crate::feed_render::ThinkingMode::Peek),
        panel_mode: Some(crate::ui::SidePanelMode::Shown(36)),
        panel_position: Some(crate::ui::SidePanelPosition::Top),
        graph_position: Some(crate::ui::GraphPosition::SidePanel),
        show_hooks: true,
        show_runtime: true,
    };
    save_to(&path, &state).unwrap();
    assert_eq!(load_from(&path), state);

    // Empty state removes the file so stale choices never resurrect.
    save_to(&path, &UiState::default()).unwrap();
    assert!(!path.exists());
    assert_eq!(load_from(&path), UiState::default());

    let _ = std::fs::remove_dir_all(&dir);
}
