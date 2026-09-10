//! Composer text + display state: the textarea accessors every handler reads,
//! the Ctrl+O / Ctrl+T display toggles, and the best-effort persistence of
//! those switches to `ui-state.toml`.

use crate::ui::App;
use crate::ui::render_utils::new_textarea;

impl App {
    /// Ctrl+O: cycle the thinking rendering mode Full → Peek → Hidden → Full.
    pub(in crate::ui) fn cycle_thinking_mode(&mut self) {
        use crate::feed_render::ThinkingMode;
        self.thinking_mode = match self.thinking_mode {
            ThinkingMode::Full => ThinkingMode::Peek,
            ThinkingMode::Peek => ThinkingMode::Hidden,
            ThinkingMode::Hidden => ThinkingMode::Full,
        };
    }

    /// Ctrl+T: expand/collapse tool results in the feed.
    pub(in crate::ui) fn toggle_tool_outputs(&mut self) {
        self.tools_expanded = !self.tools_expanded;
    }

    pub(in crate::ui) fn input_text(&self) -> String {
        self.input.text().to_string()
    }

    pub(in crate::ui) fn input_is_single_line(&self) -> bool {
        self.input_display_lines() <= 1
    }

    pub(in crate::ui) fn clear_input(&mut self) {
        self.input = new_textarea();
        self.completions.clear();
        self.completion_idx = 0;
        self.completion_scroll = 0;
    }

    pub(in crate::ui) fn set_input(&mut self, text: &str) {
        let mut input = new_textarea();
        input.insert_str(text);
        self.input = input;
        self.refresh_completions();
    }

    /// Persist the user-facing display switches to `ui-state.toml`
    /// (Ctrl+O thinking mode + `/side-panel` panel mode/position). Best
    /// effort — failures only warn.
    pub(in crate::ui) fn persist_ui_state(&self) {
        crate::ui_state::save(&crate::ui_state::UiState {
            thinking_mode: Some(self.thinking_mode),
            panel_mode: Some(self.side_panel_mode),
            panel_position: Some(self.side_panel_position),
            graph_position: Some(self.graph_position),
            show_hooks: self.show_hooks,
            show_runtime: self.show_runtime,
        });
    }
}
