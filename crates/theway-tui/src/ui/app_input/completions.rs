//! Slash-command completion popup (issues #37/#46/#95/#110): the completion
//! candidate list, its fixed render window that slides with the highlight,
//! and Tab / ↑ / ↓ / Enter handling over it.

use std::collections::HashSet;

use theway_transport::transport::SlashCompleter;

use crate::ui::App;
use crate::ui::COMPLETION_POPUP_MAX;
use crate::ui::collect_slash_commands;
use crate::ui::render_utils::new_textarea;

impl App {
    pub(in crate::ui) fn refresh_completions(&mut self) {
        let mut commands = collect_slash_commands(
            &self.registry,
            &self.latest.sidebar.skills.items,
            &self.latest.sidebar.commands,
            &self.latest.sidebar.mcp.tool_names,
        );
        commands.extend(
            self.latest
                .extensions
                .commands
                .iter()
                .map(|command| format!("/ext:{}", command.name)),
        );
        self.completer = SlashCompleter::from_commands(commands);
        self.completions = if self.input_is_single_line() {
            let mut matches = self.completer.matches(&self.input_text());
            // Issue #95/#110: a bare "/" matches every command; skill entries
            // (shortcut or skill:: fallback) would land far below the popup's
            // visible window. Surface them first so the loaded skills are
            // immediately visible — each skill has exactly one entry.
            if self.input_text().trim() == "/" {
                let skill_entries: HashSet<String> = self
                    .latest
                    .sidebar
                    .skills
                    .items
                    .iter()
                    .filter(|skill| skill.enabled)
                    .flat_map(|skill| {
                        let mut entries = vec![format!("/skill::{}", skill.name)];
                        if let Some(shortcut) = skill.name.split('/').next() {
                            entries.push(format!("/{shortcut}"));
                        }
                        entries
                    })
                    .collect();
                let (skills, rest): (Vec<String>, Vec<String>) = matches
                    .into_iter()
                    .partition(|entry| skill_entries.contains(entry));
                matches = skills.into_iter().chain(rest).collect();
            }
            matches
        } else {
            Vec::new()
        };
        self.completion_idx = 0;
        self.completion_scroll = 0;
    }

    /// Slide the popup window so the highlight stays inside
    /// `[completion_scroll, completion_scroll + COMPLETION_POPUP_MAX)`
    /// (issue #46): the highlight cycles over every match while the popup
    /// renders a fixed window, so moving above the top edge snaps the window
    /// up and moving past the bottom edge slides it down.
    fn sync_completion_scroll(&mut self) {
        if self.completion_idx < self.completion_scroll {
            self.completion_scroll = self.completion_idx;
        } else if self.completion_idx >= self.completion_scroll + COMPLETION_POPUP_MAX {
            self.completion_scroll = self.completion_idx - COMPLETION_POPUP_MAX + 1;
        }
    }

    pub(in crate::ui) fn cycle_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let options = self.completions.clone();
        let pick = self.completions[self.completion_idx % self.completions.len()].clone();
        self.completion_idx = (self.completion_idx + 1) % self.completions.len();
        self.sync_completion_scroll();
        // Replace just the slash token (the whole single-line input here).
        let mut input = new_textarea();
        input.insert_str(&pick);
        self.input = input;
        if options.len() > 1 {
            // Keep the original candidate set so repeated Tab cycles through visible choices.
            self.completions = options;
        } else {
            self.completions.clear();
            self.completion_idx = 0;
        }
    }

    /// ↑ with the command popup open: move the highlight up (issue #37).
    pub(in crate::ui) fn completion_prev(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        self.completion_idx =
            (self.completion_idx + self.completions.len() - 1) % self.completions.len();
        self.sync_completion_scroll();
    }

    /// ↓ with the command popup open: move the highlight down (issue #37).
    pub(in crate::ui) fn completion_next(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        self.completion_idx = (self.completion_idx + 1) % self.completions.len();
        self.sync_completion_scroll();
    }

    /// Enter with the command popup open: accept the highlighted entry into
    /// the input. The popup closes (the accepted text matches exactly); a
    /// second Enter submits it (issue #37).
    pub(in crate::ui) fn accept_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let pick = self.completions[self.completion_idx % self.completions.len()].clone();
        self.completion_scroll = 0;
        self.set_input(&pick);
        // Close the popup: `set_input` refreshes completions, so without this
        // a second Enter would re-accept the same entry instead of submitting.
        self.completions.clear();
        self.completion_idx = 0;
    }
}
