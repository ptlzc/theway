//! TUI theme: color roles + block layout + composer style (issues #43 + #49).
//!
//! [`Theme::load`] parses `${THEWAY_DIR:-$HOME/.theway}/theme.toml` once at
//! startup (`[colors]` roles, `[blocks.<kind>]` sections, `[composer]`
//! table). [`Theme::default`] mirrors the hardcoded tokyonight consts in
//! [`crate::feed_render`] / [`crate::ui::prompt_chrome`] exactly, so a build
//! without a theme file renders identically to the pre-theme build.
//!
//! The v1 parser handles the narrow TOML subset the theme file uses — section
//! headers, `key = value` with quoted strings / plain integers, `#` comments
//! (quote-aware) — without a `toml` dependency. Anything unknown or malformed
//! (unknown section/role/key, invalid hex, non-`left|right` align, invalid
//! padding) warns on stderr and keeps the current value (the default unless
//! overridden earlier); missing sections/keys and a missing file all mean
//! "default".

use std::path::{Path, PathBuf};

use ratatui::style::Color;

use crate::feed_render::{
    ASSISTANT_PREFIX_DEFAULT, ASSISTANT_TEXT_DEFAULT, THINKING_BG_DEFAULT, THINKING_TEXT_DEFAULT,
    TOOL_ARGS_DEFAULT, TOOL_ERROR_BG_DEFAULT, TOOL_ERROR_DEFAULT, TOOL_RESULT_DEFAULT,
    TOOL_RUNNING_BG_DEFAULT, TOOL_SUCCESS_BG_DEFAULT, TOOL_TITLE_DEFAULT, USER_BG_DEFAULT,
    USER_TEXT_DEFAULT,
};
use crate::ui::prompt_chrome::{
    ACCENT_USER, BG_BASE, BORDER_FOCUSED, BORDER_UNFOCUSED, TEXT_PRIMARY, TEXT_SECONDARY,
};

/// Horizontal alignment of block content (issue #49).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BlockAlign {
    /// Content hugs the left padding (the pre-theme layout).
    #[default]
    Left,
    /// Content hugs the right padding; the background still spans the full
    /// block width.
    Right,
}

/// Per-block layout (`[blocks.user]` / `[blocks.assistant]` / `[blocks.tool]`
/// / `[blocks.thinking]`).
///
/// `padding` / `align` render as part of the background fill: without a
/// background (both the section `bg` and the role background unset) the block
/// keeps the classic flush layout, so the default theme is visually identical
/// to the pre-theme render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockTheme {
    /// Explicit section background; `None` falls back to the color role
    /// (`tool_running_bg` / `tool_success_bg` / `tool_error_bg` /
    /// `thinking_bg`). Both unset → no background.
    pub bg: Option<Color>,
    /// Horizontal padding columns inside the background (default 1, `0`
    /// allowed).
    pub padding: u16,
    /// Content alignment within the block (default left).
    pub align: BlockAlign,
}

impl Default for BlockTheme {
    fn default() -> Self {
        Self {
            bg: None,
            padding: 1,
            align: BlockAlign::Left,
        }
    }
}

/// Composer chrome colors (`[composer]`), defaulting to
/// [`crate::ui::prompt_chrome`]'s pre-theme consts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComposerStyle {
    /// Brighter chrome when the input is focused.
    pub border_focused: Color,
    /// Dimmer chrome while a picker / control prompt is open.
    pub border_unfocused: Color,
    /// Focused `❯` prefix color.
    pub prefix: Color,
    /// Content text color.
    pub text: Color,
    /// Prompt background surface.
    pub bg: Color,
    /// Info-line caption color (blended toward `bg`).
    pub info_text: Color,
}

impl Default for ComposerStyle {
    fn default() -> Self {
        Self {
            border_focused: BORDER_FOCUSED,
            border_unfocused: BORDER_UNFOCUSED,
            prefix: ACCENT_USER,
            text: TEXT_PRIMARY,
            bg: BG_BASE,
            info_text: TEXT_SECONDARY,
        }
    }
}

/// Color roles (#43), block layout (#49) and composer style in one copyable
/// bundle. `FeedRenderOptions` carries it so the feed cache fingerprints
/// theme changes; `App` loads it once at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    // ── color roles (#43) ──────────────────────────────────────────────────
    pub user_text: Color,
    pub user_bg: Color,
    pub assistant_text: Option<Color>,
    pub assistant_prefix: Color,
    pub tool_title: Color,
    pub tool_args: Color,
    pub tool_result: Color,
    pub tool_error: Color,
    pub tool_running_bg: Option<Color>,
    pub tool_success_bg: Option<Color>,
    pub tool_error_bg: Option<Color>,
    pub thinking_text: Color,
    pub thinking_bg: Option<Color>,
    // ── block layout (#49) ─────────────────────────────────────────────────
    /// `[blocks.user]` section. Parsed for theme completeness; the v1
    /// renderer applies block layout to tool/thinking blocks only.
    #[allow(dead_code)] // TODO(#49): apply user/assistant block layout in the renderer.
    pub user: BlockTheme,
    /// `[blocks.assistant]` section (same v1 render scope as `user`).
    #[allow(dead_code)] // TODO(#49): apply user/assistant block layout in the renderer.
    pub assistant: BlockTheme,
    pub tool: BlockTheme,
    pub thinking: BlockTheme,
    // ── composer (#49) ─────────────────────────────────────────────────────
    pub composer: ComposerStyle,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            user_text: USER_TEXT_DEFAULT,
            user_bg: USER_BG_DEFAULT,
            assistant_text: ASSISTANT_TEXT_DEFAULT,
            assistant_prefix: ASSISTANT_PREFIX_DEFAULT,
            tool_title: TOOL_TITLE_DEFAULT,
            tool_args: TOOL_ARGS_DEFAULT,
            tool_result: TOOL_RESULT_DEFAULT,
            tool_error: TOOL_ERROR_DEFAULT,
            tool_running_bg: TOOL_RUNNING_BG_DEFAULT,
            tool_success_bg: TOOL_SUCCESS_BG_DEFAULT,
            tool_error_bg: TOOL_ERROR_BG_DEFAULT,
            thinking_text: THINKING_TEXT_DEFAULT,
            thinking_bg: THINKING_BG_DEFAULT,
            user: BlockTheme::default(),
            assistant: BlockTheme::default(),
            tool: BlockTheme::default(),
            thinking: BlockTheme::default(),
            composer: ComposerStyle::default(),
        }
    }
}

impl Theme {
    /// Load the theme from `${THEWAY_DIR:-$HOME/.theway}/theme.toml`. A
    /// missing or unreadable file is the documented default (no warning).
    pub fn load() -> Self {
        Self::load_from(&theme_toml_path())
    }

    /// Parse `path` when it exists; any read error (missing file, no
    /// permissions) → [`Theme::default`].
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Parse the v1 theme.toml subset onto a default theme. Unknown sections
    /// / keys / roles, invalid hex, unknown align and invalid padding warn on
    /// stderr and keep the current value; everything missing stays default.
    pub fn parse(text: &str) -> Self {
        let mut theme = Theme::default();
        let mut section = "";
        for (idx, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = header.trim();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warn(&format!(
                    "line {}: expected `key = value`, got {line:?}",
                    idx + 1
                ));
                continue;
            };
            let key = key.trim();
            let mut value = value.trim();
            if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                value = &value[1..value.len() - 1];
            }
            match section {
                "colors" => apply_color_role(&mut theme, key, value),
                "composer" => apply_composer_key(&mut theme.composer, key, value),
                "blocks.user" => apply_block_key(&mut theme.user, "user", key, value),
                "blocks.assistant" => {
                    apply_block_key(&mut theme.assistant, "assistant", key, value)
                }
                "blocks.tool" => apply_block_key(&mut theme.tool, "tool", key, value),
                "blocks.thinking" => apply_block_key(&mut theme.thinking, "thinking", key, value),
                "" => warn(&format!("line {}: key outside any [section]", idx + 1)),
                unknown => warn(&format!(
                    "line {}: unknown section {unknown:?} — ignored",
                    idx + 1
                )),
            }
        }
        theme
    }
}

/// `${THEWAY_DIR:-$HOME/.theway}/theme.toml`, matching the runtime-state
/// layout documented in AGENTS.md.
fn theme_toml_path() -> PathBuf {
    let base = std::env::var("THEWAY_DIR")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".theway")))
        .unwrap_or_else(|_| PathBuf::from(".theway"));
    base.join("theme.toml")
}

/// Strip a `#` comment, honoring quotes so `key = "#010203"` survives.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn warn(msg: &str) {
    eprintln!("theway theme: {msg}");
}

/// `#RRGGBB` → [`Color::Rgb`]; anything else (missing `#`, bad hex,
/// unquoted values eaten by comment stripping) → `None` for the caller to
/// warn on.
fn parse_color(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(Color::from_u32)
}

fn set_color(slot: &mut Color, key: &str, value: &str) {
    match parse_color(value) {
        Some(color) => *slot = color,
        None => warn(&format!(
            "colors.{key}: invalid hex {value:?} — keeping the current value"
        )),
    }
}

fn set_opt_color(slot: &mut Option<Color>, key: &str, value: &str) {
    match parse_color(value) {
        Some(color) => *slot = Some(color),
        None => warn(&format!(
            "colors.{key}: invalid hex {value:?} — keeping the current value"
        )),
    }
}

fn apply_color_role(theme: &mut Theme, key: &str, value: &str) {
    match key {
        "user_text" => set_color(&mut theme.user_text, key, value),
        "user_bg" => set_color(&mut theme.user_bg, key, value),
        "assistant_text" => set_opt_color(&mut theme.assistant_text, key, value),
        "assistant_prefix" => set_color(&mut theme.assistant_prefix, key, value),
        "tool_title" => set_color(&mut theme.tool_title, key, value),
        "tool_args" => set_color(&mut theme.tool_args, key, value),
        "tool_result" => set_color(&mut theme.tool_result, key, value),
        "tool_error" => set_color(&mut theme.tool_error, key, value),
        "tool_running_bg" => set_opt_color(&mut theme.tool_running_bg, key, value),
        "tool_success_bg" => set_opt_color(&mut theme.tool_success_bg, key, value),
        "tool_error_bg" => set_opt_color(&mut theme.tool_error_bg, key, value),
        "thinking_text" => set_color(&mut theme.thinking_text, key, value),
        "thinking_bg" => set_opt_color(&mut theme.thinking_bg, key, value),
        unknown => warn(&format!("colors.{unknown}: unknown role — ignored")),
    }
}

fn apply_block_key(block: &mut BlockTheme, name: &str, key: &str, value: &str) {
    match key {
        "bg" => match parse_color(value) {
            Some(color) => block.bg = Some(color),
            None => warn(&format!(
                "blocks.{name}.bg: invalid hex {value:?} — keeping the current value"
            )),
        },
        "padding" => match value.parse::<u16>() {
            Ok(padding) => block.padding = padding,
            Err(_) => warn(&format!(
                "blocks.{name}.padding: invalid padding {value:?} — keeping the current value"
            )),
        },
        "align" => match value {
            "left" => block.align = BlockAlign::Left,
            "right" => block.align = BlockAlign::Right,
            other => warn(&format!(
                "blocks.{name}.align: unknown alignment {other:?} — keeping the current value"
            )),
        },
        unknown => warn(&format!("blocks.{name}.{unknown}: unknown key — ignored")),
    }
}

fn apply_composer_key(composer: &mut ComposerStyle, key: &str, value: &str) {
    let slot = match key {
        "border_focused" => Some(&mut composer.border_focused),
        "border_unfocused" => Some(&mut composer.border_unfocused),
        "prefix" => Some(&mut composer.prefix),
        "text" => Some(&mut composer.text),
        "bg" => Some(&mut composer.bg),
        "info_text" => Some(&mut composer.info_text),
        unknown => {
            warn(&format!("composer.{unknown}: unknown key — ignored"));
            None
        }
    };
    let Some(slot) = slot else { return };
    match parse_color(value) {
        Some(color) => *slot = color,
        None => warn(&format!(
            "composer.{key}: invalid hex {value:?} — keeping the current value"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_render;
    use crate::ui::prompt_chrome;

    #[test]
    fn default_matches_hardcoded_colors() {
        // No theme file → every role/composer color equals the pre-theme
        // hardcoded const so the visuals stay identical.
        let t = Theme::default();
        assert_eq!(t.user_text, feed_render::USER_TEXT_DEFAULT);
        assert_eq!(t.user_bg, feed_render::USER_BG_DEFAULT);
        assert_eq!(t.assistant_text, feed_render::ASSISTANT_TEXT_DEFAULT);
        assert_eq!(t.assistant_prefix, feed_render::ASSISTANT_PREFIX_DEFAULT);
        assert_eq!(t.tool_title, feed_render::TOOL_TITLE_DEFAULT);
        assert_eq!(t.tool_args, feed_render::TOOL_ARGS_DEFAULT);
        assert_eq!(t.tool_result, feed_render::TOOL_RESULT_DEFAULT);
        assert_eq!(t.tool_error, feed_render::TOOL_ERROR_DEFAULT);
        assert_eq!(t.tool_running_bg, feed_render::TOOL_RUNNING_BG_DEFAULT);
        assert_eq!(t.tool_success_bg, feed_render::TOOL_SUCCESS_BG_DEFAULT);
        assert_eq!(t.tool_error_bg, feed_render::TOOL_ERROR_BG_DEFAULT);
        assert_eq!(t.thinking_text, feed_render::THINKING_TEXT_DEFAULT);
        assert_eq!(t.thinking_bg, feed_render::THINKING_BG_DEFAULT);
        assert_eq!(t.composer, ComposerStyle::default());
        assert_eq!(t.composer.border_focused, prompt_chrome::BORDER_FOCUSED);
        assert_eq!(t.composer.border_unfocused, prompt_chrome::BORDER_UNFOCUSED);
        assert_eq!(t.composer.prefix, prompt_chrome::ACCENT_USER);
        assert_eq!(t.composer.text, prompt_chrome::TEXT_PRIMARY);
        assert_eq!(t.composer.bg, prompt_chrome::BG_BASE);
        assert_eq!(t.composer.info_text, prompt_chrome::TEXT_SECONDARY);
        for block in [t.user, t.assistant, t.tool, t.thinking] {
            assert_eq!(block.bg, None);
            assert_eq!(block.padding, 1);
            assert_eq!(block.align, BlockAlign::Left);
        }
    }

    #[test]
    fn parse_applies_color_block_and_composer_overrides() {
        let theme = Theme::parse(
            r##"
# full override sample
[colors]
user_text = "#010203"
assistant_text = "#040506"
tool_running_bg = "#0a0b0c"
thinking_bg = "#0d0e0f"

[blocks.tool]
bg = "#111213"
padding = 2
align = "right"

[blocks.thinking]
padding = 0
align = "left"

[composer]
border_focused = "#202122"
prefix = "#232425"
bg = "#262728"
"##,
        );
        assert_eq!(theme.user_text, Color::Rgb(1, 2, 3));
        assert_eq!(theme.assistant_text, Some(Color::Rgb(4, 5, 6)));
        assert_eq!(theme.tool_running_bg, Some(Color::Rgb(10, 11, 12)));
        assert_eq!(theme.thinking_bg, Some(Color::Rgb(13, 14, 15)));
        assert_eq!(theme.tool.bg, Some(Color::Rgb(17, 18, 19)));
        assert_eq!(theme.tool.padding, 2);
        assert_eq!(theme.tool.align, BlockAlign::Right);
        assert_eq!(theme.thinking.padding, 0);
        assert_eq!(theme.thinking.align, BlockAlign::Left);
        assert_eq!(theme.composer.border_focused, Color::Rgb(32, 33, 34));
        assert_eq!(theme.composer.prefix, Color::Rgb(35, 36, 37));
        assert_eq!(theme.composer.bg, Color::Rgb(38, 39, 40));
        // Keys the file does not touch keep their defaults.
        let d = Theme::default();
        assert_eq!(theme.tool_title, d.tool_title);
        assert_eq!(theme.user.bg, d.user.bg);
        assert_eq!(theme.composer.border_unfocused, d.composer.border_unfocused);
    }

    #[test]
    fn parse_unknown_role_section_and_key_fall_back() {
        let theme = Theme::parse(
            "[colors]\nuser_text = \"#010203\"\nwat = \"#ffffff\"\n\
             [blocks.foo]\nbg = \"#999999\"\n\
             [blocks.tool]\nwobble = 3\n\
             [composer]\nwut = \"#888888\"\n",
        );
        let d = Theme::default();
        assert_eq!(theme.user_text, Color::Rgb(1, 2, 3));
        assert_eq!(theme.tool_title, d.tool_title);
        assert_eq!(theme.tool.bg, d.tool.bg);
        assert_eq!(theme.tool.padding, d.tool.padding);
        assert_eq!(theme.composer, d.composer);
    }

    #[test]
    fn parse_invalid_hex_falls_back() {
        let theme = Theme::parse(
            "[colors]\nuser_text = \"nope\"\nuser_bg = \"#12345\"\nassistant_prefix = \"#zzzzzz\"\n\
             [blocks.tool]\nbg = \"343541\"\n",
        );
        let d = Theme::default();
        assert_eq!(theme.user_text, d.user_text);
        assert_eq!(theme.user_bg, d.user_bg);
        assert_eq!(theme.assistant_prefix, d.assistant_prefix);
        // `343541` without the `#` is not a valid hex color either.
        assert_eq!(theme.tool.bg, None);
    }

    #[test]
    fn parse_invalid_align_and_padding_fall_back() {
        let theme = Theme::parse(
            "[blocks.tool]\npadding = 2\nalign = \"center\"\n\
             [blocks.thinking]\npadding = -1\nalign = \"right\"\n",
        );
        assert_eq!(theme.tool.padding, 2);
        assert_eq!(theme.tool.align, BlockAlign::Left);
        assert_eq!(theme.thinking.padding, 1);
        assert_eq!(theme.thinking.align, BlockAlign::Right);
    }

    #[test]
    fn parse_missing_sections_and_keys_keep_defaults() {
        let theme =
            Theme::parse("[colors]\nuser_text = \"#010203\"\n[blocks.tool]\nbg = \"#0a0b0c\"\n");
        let d = Theme::default();
        assert_eq!(theme.user_text, Color::Rgb(1, 2, 3));
        // Missing padding key in a present section → default 1.
        assert_eq!(theme.tool.padding, 1);
        assert_eq!(theme.tool.align, BlockAlign::Left);
        assert_eq!(theme.tool.bg, Some(Color::Rgb(10, 11, 12)));
        // Missing sections entirely → defaults.
        assert_eq!(theme.thinking, d.thinking);
        assert_eq!(theme.user, d.user);
        assert_eq!(theme.assistant, d.assistant);
        assert_eq!(theme.composer, d.composer);
        assert_eq!(theme.user_bg, d.user_bg);
    }

    #[test]
    fn parse_ignores_comments_and_blank_lines() {
        let theme = Theme::parse(
            "# header comment\n\n[colors]  # section comment\nuser_text = \"#010203\" # trailing\n",
        );
        assert_eq!(theme.user_text, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn parse_rejects_key_outside_section() {
        let theme = Theme::parse("user_text = \"#010203\"\n");
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn load_missing_file_returns_default() {
        let missing = std::env::temp_dir().join(format!(
            "theway-theme-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(Theme::load_from(&missing), Theme::default());
    }

    #[test]
    fn load_from_reads_theme_toml_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.toml");
        std::fs::write(
            &path,
            "[blocks.tool]\nbg = \"#010203\"\npadding = 0\nalign = \"right\"\n",
        )
        .unwrap();
        let theme = Theme::load_from(&path);
        assert_eq!(theme.tool.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(theme.tool.padding, 0);
        assert_eq!(theme.tool.align, BlockAlign::Right);
    }
}
