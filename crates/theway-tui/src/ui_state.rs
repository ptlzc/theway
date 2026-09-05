//! TUI-local UI state persistence: `${THEWAY_DIR}/ui-state.toml`.
//!
//! Carries the display switches the user picks with keybindings/menus so they
//! survive restarts:
//!
//! ```toml
//! [feed]
//! thinking_mode = "peek"      # full | peek | hidden (Ctrl+O cycles)
//!
//! [panel]
//! mode = "shown"              # auto | shown | hidden (/side-panel › Toggle)
//! position = "left"           # top | bottom | left | right (/side-panel › Position)
//!
//! [graph]
//! position = "side-panel"     # composer-top | side-panel (/graph › Position)
//! ```
//!
//! Missing, unreadable, or malformed files fall back to defaults silently —
//! the same contract as `theme.toml`. Writes are atomic (temp file + rename)
//! and failures only warn.

use std::path::Path;

use crate::feed_render::ThinkingMode;
use crate::ui::{GraphPosition, SidePanelMode, SidePanelPosition, TRIGGER_PANEL_WIDTH};

#[cfg(test)]
static TEST_STATE_PATH: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Redirect the persistence path for hermetic fixtures; `None` restores the
/// real path.
#[cfg(test)]
pub(crate) fn set_state_path_for_tests(path: Option<std::path::PathBuf>) {
    *TEST_STATE_PATH.lock().unwrap() = path;
}

fn state_path() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_STATE_PATH.lock().unwrap().clone() {
        return path;
    }
    theway_transport::config::base_dir().join("ui-state.toml")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UiState {
    pub thinking_mode: Option<ThinkingMode>,
    pub panel_mode: Option<SidePanelMode>,
    pub panel_position: Option<SidePanelPosition>,
    pub graph_position: Option<GraphPosition>,
}

/// Load from the default path (`${THEWAY_DIR}/ui-state.toml`).
pub(crate) fn load() -> UiState {
    load_from(&state_path())
}

/// Parse `path` when it exists; any read/parse error → defaults.
pub(crate) fn load_from(path: &Path) -> UiState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return UiState::default();
    };
    parse(&text)
}

fn parse(text: &str) -> UiState {
    let mut state = UiState::default();
    let table: toml::map::Map<String, toml::Value> = match text.parse() {
        Ok(table) => table,
        Err(_) => return state,
    };
    if let Some(feed) = table.get("feed").and_then(toml::Value::as_table) {
        if let Some(mode) = feed.get("thinking_mode").and_then(toml::Value::as_str) {
            state.thinking_mode = parse_thinking_mode(mode);
        }
    }
    if let Some(panel) = table.get("panel").and_then(toml::Value::as_table) {
        if let Some(mode) = panel.get("mode").and_then(toml::Value::as_str) {
            state.panel_mode = parse_panel_mode(mode);
        }
        if let Some(position) = panel.get("position").and_then(toml::Value::as_str) {
            state.panel_position = parse_panel_position(position);
        }
    }
    if let Some(graph) = table.get("graph").and_then(toml::Value::as_table) {
        if let Some(position) = graph.get("position").and_then(toml::Value::as_str) {
            state.graph_position = parse_graph_position(position);
        }
    }
    state
}

fn parse_thinking_mode(mode: &str) -> Option<ThinkingMode> {
    match mode {
        "full" => Some(ThinkingMode::Full),
        "peek" => Some(ThinkingMode::Peek),
        "hidden" => Some(ThinkingMode::Hidden),
        _ => None,
    }
}

fn parse_panel_mode(mode: &str) -> Option<SidePanelMode> {
    match mode {
        "auto" => Some(SidePanelMode::Auto),
        "shown" => Some(SidePanelMode::Shown(TRIGGER_PANEL_WIDTH)),
        "hidden" => Some(SidePanelMode::Hidden),
        _ => None,
    }
}

fn parse_graph_position(position: &str) -> Option<GraphPosition> {
    match position {
        "composer-top" => Some(GraphPosition::ComposerTop),
        "side-panel" => Some(GraphPosition::SidePanel),
        _ => None,
    }
}

fn parse_panel_position(position: &str) -> Option<SidePanelPosition> {
    match position {
        "top" => Some(SidePanelPosition::Top),
        "bottom" => Some(SidePanelPosition::Bottom),
        "left" => Some(SidePanelPosition::Left),
        "right" => Some(SidePanelPosition::Right),
        _ => None,
    }
}

fn thinking_mode_str(mode: ThinkingMode) -> &'static str {
    match mode {
        ThinkingMode::Full => "full",
        ThinkingMode::Peek => "peek",
        ThinkingMode::Hidden => "hidden",
    }
}

fn panel_mode_str(mode: SidePanelMode) -> &'static str {
    match mode {
        SidePanelMode::Auto => "auto",
        SidePanelMode::Shown(_) => "shown",
        SidePanelMode::Hidden => "hidden",
    }
}

fn panel_position_str(position: SidePanelPosition) -> &'static str {
    match position {
        SidePanelPosition::Top => "top",
        SidePanelPosition::Bottom => "bottom",
        SidePanelPosition::Left => "left",
        SidePanelPosition::Right => "right",
    }
}

/// Serialize the non-empty fields (mirrors [`UiState`]).
pub(crate) fn render(state: &UiState) -> String {
    let mut out = String::new();
    let feed_fields = state
        .thinking_mode
        .map(|mode| format!("thinking_mode = \"{}\"", thinking_mode_str(mode)));
    if let Some(fields) = feed_fields {
        out.push_str("[feed]\n");
        out.push_str(&fields);
        out.push('\n');
    }
    let panel_fields = [
        state
            .panel_mode
            .map(|mode| format!("mode = \"{}\"", panel_mode_str(mode))),
        state
            .panel_position
            .map(|position| format!("position = \"{}\"", panel_position_str(position))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !panel_fields.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[panel]\n");
        for field in panel_fields {
            out.push_str(&field);
            out.push('\n');
        }
    }
    if let Some(position) = state.graph_position {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[graph]\n");
        let label = match position {
            GraphPosition::ComposerTop => "composer-top",
            GraphPosition::SidePanel => "side-panel",
        };
        out.push_str(&format!("position = \"{label}\"\n"));
    }
    out
}

/// Persist to the default path. Failures (missing dir, permissions) warn and
/// never propagate — UI state is convenience, not data.
pub(crate) fn save(state: &UiState) {
    if let Err(error) = save_to(&state_path(), state) {
        tracing::warn!("save ui-state: {error}");
    }
}

/// Atomic write: temp file in the same directory + rename.
pub(crate) fn save_to(path: &Path, state: &UiState) -> std::io::Result<()> {
    let text = render(state);
    if text.is_empty() {
        // Nothing to persist: drop any stale file so a future load starts
        // clean instead of resurrecting old choices.
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".ui-state.tmp.{}",
        std::process::id().wrapping_add(text.len() as u32)
    ));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
// Test files live in `tests/ui_state.rs`, pulled in by path so they keep
// unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("ui_state");
