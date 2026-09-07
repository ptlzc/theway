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
//! show_hooks = true           # render the Hooks section (default: hidden)
//! show_runtime = true         # render the Runtime section (default: hidden)
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
    /// Render the side-panel `Hooks` section (hidden by default; the section
    /// is diagnostic detail, opt in via `[ui.panel] show_hooks = true`).
    pub show_hooks: bool,
    /// Render the side-panel `Runtime` section (hidden by default; opt in via
    /// `[ui.panel] show_runtime = true`).
    pub show_runtime: bool,
}

/// Load the persisted UI state.
///
/// Reads a `[ui]` table from `config.toml` (the controller-owned config file)
/// when one is present; otherwise falls back to the legacy `ui-state.toml`.
pub(crate) fn load() -> UiState {
    let config_path = crate::config_payload::config_path(None);
    if let Ok(text) = std::fs::read_to_string(&config_path) {
        if let Ok(table) = text.parse::<toml::Table>() {
            if table.contains_key("ui") {
                return parse_namespaced(&text);
            }
        }
    }
    load_from(&state_path())
}

/// Parse `config.toml` text, honoring the `[ui]` namespace when present.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn load_from_config_text(text: &str) -> UiState {
    parse_namespaced(text)
}

/// Parse `path` when it exists; any read/parse error → defaults.
pub(crate) fn load_from(path: &Path) -> UiState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return UiState::default();
    };
    parse(&text)
}

fn parse(text: &str) -> UiState {
    let table: toml::map::Map<String, toml::Value> = match text.parse() {
        Ok(table) => table,
        Err(_) => return UiState::default(),
    };
    read_ui_sections(&table, UiState::default())
}

/// Parse text that may carry a `[ui]` namespace: when a `[ui]` table is
/// present, read `ui.feed`/`ui.panel`/`ui.graph`; otherwise fall back to the
/// top-level `[feed]`/`[panel]`/`[graph]` layout (graceful).
fn parse_namespaced(text: &str) -> UiState {
    let table: toml::map::Map<String, toml::Value> = match text.parse() {
        Ok(table) => table,
        Err(_) => return UiState::default(),
    };
    match table.get("ui") {
        Some(ui) => parse_ui_section(ui),
        None => read_ui_sections(&table, UiState::default()),
    }
}

/// Read `feed.thinking_mode`, `panel.mode`, `panel.position` and
/// `graph.position` out of `container` — the same field parsing whether the
/// tables live at the top level (legacy `ui-state.toml`) or under `[ui]`
/// (`config.toml`).
fn read_ui_sections(
    container: &toml::map::Map<String, toml::Value>,
    mut state: UiState,
) -> UiState {
    if let Some(feed) = container.get("feed").and_then(toml::Value::as_table) {
        if let Some(mode) = feed.get("thinking_mode").and_then(toml::Value::as_str) {
            state.thinking_mode = parse_thinking_mode(mode);
        }
    }
    if let Some(panel) = container.get("panel").and_then(toml::Value::as_table) {
        if let Some(mode) = panel.get("mode").and_then(toml::Value::as_str) {
            state.panel_mode = parse_panel_mode(mode);
        }
        if let Some(position) = panel.get("position").and_then(toml::Value::as_str) {
            state.panel_position = parse_panel_position(position);
        }
        state.show_hooks = panel
            .get("show_hooks")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        state.show_runtime = panel
            .get("show_runtime")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
    }
    if let Some(graph) = container.get("graph").and_then(toml::Value::as_table) {
        if let Some(position) = graph.get("position").and_then(toml::Value::as_str) {
            state.graph_position = parse_graph_position(position);
        }
    }
    state
}

/// Read the UI state from a `[ui]` sub-table (e.g. the value of `config.toml`'s
/// `ui` key).
fn parse_ui_section(ui: &toml::Value) -> UiState {
    ui.as_table()
        .map(|table| read_ui_sections(table, UiState::default()))
        .unwrap_or_default()
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
        state.show_hooks.then(|| "show_hooks = true".to_string()),
        state
            .show_runtime
            .then(|| "show_runtime = true".to_string()),
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
