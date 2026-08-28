//! TUI theme: color roles + block layout + composer style + feed rhythm
//! (issues #43 + #49 + #30).
//!
//! [`Theme::load`] parses `${THEWAY_DIR:-$HOME/.theway}/theme.toml` once at
//! startup. Sections: `[colors]` roles, `[blocks.<kind>]` layout,
//! `[composer]` chrome, `[feed]` vertical rhythm, `[palette]` named colors.
//! [`Theme::default`] mirrors the hardcoded tokyonight consts in
//! [`crate::feed_render`] / [`crate::ui::prompt_chrome`] exactly, so a build
//! without a theme file renders identically to the pre-theme build.
//!
//! v2 (issue #30) upgrades parsing to the `toml` crate while keeping the v1
//! key set and semantics byte-identical. New in v2:
//! - `[palette]` — named colors referenced from any color slot as `p:name`.
//! - Extended color literals: short hex `#7AF`, ANSI names (`red`,
//!   `lightBlue`, `default`), 256-palette indexes (`"146"`), and
//!   `transparent` / `none` to clear optional (background) slots.
//! - `[feed] gap` / `separator` / `separator_style` — the inter-block
//!   vertical rhythm that v1 could not express.
//!
//! Forgiving posture (kept from v1): unknown sections/keys, invalid hex and
//! invalid values warn on stderr and keep the current value (the default
//! unless overridden earlier); missing sections/keys and a missing file all
//! mean "default".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::Color;
use toml::Table as TomlTable;

use crate::feed_render::{
    ASSISTANT_PREFIX_DEFAULT, ASSISTANT_TEXT_DEFAULT, THINKING_BG_DEFAULT, THINKING_TEXT_DEFAULT,
    TOOL_ARGS_DEFAULT, TOOL_ERROR_BG_DEFAULT, TOOL_ERROR_DEFAULT, TOOL_RESULT_DEFAULT,
    TOOL_RUNNING_BG_DEFAULT, TOOL_SUCCESS_BG_DEFAULT, TOOL_TITLE_DEFAULT, USER_BG_DEFAULT,
    USER_TEXT_DEFAULT,
};
use crate::ui::prompt_chrome::{
    ACCENT_USER, BG_BASE, BORDER_FOCUSED, BORDER_UNFOCUSED, GRAY, GRAY_DIM, TEXT_PRIMARY,
    TEXT_SECONDARY,
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

/// Block edge border weight (`[blocks.<kind>] border_top/border_bottom`,
/// issue #31).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BlockBorder {
    /// No border line.
    #[default]
    None,
    /// Single-width line glyph `─`.
    Thin,
    /// Heavy line glyph `━`.
    Thick,
}

/// Per-block layout (`[blocks.user]` / `[blocks.assistant]` / `[blocks.tool]`
/// / `[blocks.thinking]`).
///
/// `padding` / `align` render as part of the background fill: without a
/// background (both the section `bg` and the role background unset) the block
/// keeps the classic flush layout, so the default theme is visually identical
/// to the pre-theme render.
///
/// `margin_top` / `margin_bottom` add blank rows above/below the block
/// (independent of `[feed] gap` — both accumulate); `border_top` /
/// `border_bottom` draw a full-width styled line inside the margins
/// (issue #31).
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
    /// Extra blank rows above the block (default 0).
    pub margin_top: u16,
    /// Extra blank rows below the block (default 0).
    pub margin_bottom: u16,
    /// Top edge border (default none).
    pub border_top: BlockBorder,
    /// Bottom edge border (default none).
    pub border_bottom: BlockBorder,
    /// Border line color.
    pub border_style: Color,
}

impl Default for BlockTheme {
    fn default() -> Self {
        Self {
            bg: None,
            padding: 1,
            align: BlockAlign::Left,
            margin_top: 0,
            margin_bottom: 0,
            border_top: BlockBorder::None,
            border_bottom: BlockBorder::None,
            border_style: crate::ui::prompt_chrome::GRAY_DIM,
        }
    }
}

/// Feed vertical rhythm (`[feed]`, issue #30): how much space separates
/// blocks. `should_separate` (transport feed model) still decides WHERE a
/// gap goes; this decides HOW MUCH.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedTheme {
    /// Blank lines between blocks (default 1; `0` = flush).
    pub gap: u16,
    /// When set, a full-width line of this glyph replaces the last blank
    /// row of the gap (e.g. `─`); `None` = pure blank lines.
    pub separator: Option<char>,
    /// Color of the separator line.
    pub separator_style: Color,
}

impl Default for FeedTheme {
    fn default() -> Self {
        Self {
            gap: 1,
            separator: None,
            separator_style: GRAY_DIM,
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
    /// Empty-input placeholder text color (issue #31).
    pub placeholder: Color,
    /// Hint line below the input box (issue #31).
    pub hint: Color,
    /// Input cursor color where the renderer draws one (issue #31).
    pub cursor: Color,
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
            placeholder: GRAY,
            hint: Color::DarkGray,
            cursor: TEXT_PRIMARY,
        }
    }
}

/// Screen-level viewport inset (`[screen]`): how far the whole UI sits from
/// the terminal edges. All sides default to `0` (flush — the pre-theme
/// behavior). `margin = N` sets all four sides; per-side
/// `margin_top/right/bottom/left` keys override individual sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenStyle {
    pub margin_top: u16,
    pub margin_right: u16,
    pub margin_bottom: u16,
    pub margin_left: u16,
}

impl ScreenStyle {
    /// Inset `rect` by the four margins. Saturating: a margin larger than the
    /// terminal collapses the area to zero rather than underflowing.
    pub fn inset(self, rect: Rect) -> Rect {
        Rect {
            x: rect.x + self.margin_left,
            y: rect.y + self.margin_top,
            width: rect
                .width
                .saturating_sub(self.margin_left + self.margin_right),
            height: rect
                .height
                .saturating_sub(self.margin_top + self.margin_bottom),
        }
    }
}

/// Status band (`[statusbar]`, issue #31): the busy/ready line and the
/// working cluster above the feed. Defaults match `ui/app/status.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusbarStyle {
    /// Band background; `None` = terminal default.
    pub bg: Option<Color>,
    /// Idle/ready line color.
    pub fg: Color,
    /// Busy accent (spinner glyph).
    pub accent: Color,
    /// Error emphasis.
    pub error: Color,
    /// Busy/working label color.
    pub busy: Color,
}

impl Default for StatusbarStyle {
    fn default() -> Self {
        Self {
            bg: None,
            fg: Color::DarkGray,
            accent: Color::Yellow,
            error: Color::Red,
            busy: Color::Gray,
        }
    }
}

/// Interactive picker popups (`[picker]`, issue #31): model/fork/resume
/// pickers and the status-panel menu. Defaults match `ui/app/render.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickerStyle {
    /// Popup background; `None` = terminal default.
    pub bg: Option<Color>,
    /// Row text color.
    pub fg: Color,
    /// Selected-row background.
    pub highlight_bg: Color,
    /// Selected-row text color.
    pub highlight_fg: Color,
    /// Popup title color.
    pub title: Color,
    /// Dim/secondary text.
    pub dim: Color,
}

impl Default for PickerStyle {
    fn default() -> Self {
        Self {
            bg: None,
            fg: Color::Cyan,
            highlight_bg: Color::Cyan,
            highlight_fg: Color::Black,
            title: Color::Yellow,
            dim: Color::DarkGray,
        }
    }
}

/// Side panel (`[sidebar]`, issue #31): the automation/trigger panel.
/// Defaults match `ui/app/panel.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarStyle {
    /// Panel background; `None` = terminal default.
    pub bg: Option<Color>,
    /// Border + plain row color.
    pub fg: Color,
    /// Block title (" Automation ").
    pub heading: Color,
    /// Section headings (Extensions / Skills / Triggers / …).
    pub section: Color,
    /// Positive/badge emphasis (enabled, achieved).
    pub badge: Color,
    /// Warning emphasis (disabled counts, reload pending).
    pub warn: Color,
    /// Dim/summary text.
    pub muted: Color,
}

impl Default for SidebarStyle {
    fn default() -> Self {
        Self {
            bg: None,
            fg: Color::DarkGray,
            heading: Color::Magenta,
            section: Color::Cyan,
            badge: Color::Green,
            warn: Color::Yellow,
            muted: Color::DarkGray,
        }
    }
}

/// DAG band (`[dag_band]`, issue #31): the graph status band above the feed.
/// Defaults match `ui/dag_band.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DagBandStyle {
    /// Band background; `None` = terminal default.
    pub bg: Option<Color>,
    /// Plain text color (unknown states, tail labels).
    pub fg: Color,
    /// Succeeded/ok state.
    pub ok: Color,
    /// Failed state.
    pub failed: Color,
    /// Cancelled state.
    pub cancelled: Color,
    /// Running state.
    pub running: Color,
    /// Ready-to-run state.
    pub pending: Color,
    /// Skipped state.
    pub skipped: Color,
    /// Separators and edges.
    pub edge: Color,
    /// Run header title.
    pub title: Color,
}

impl Default for DagBandStyle {
    fn default() -> Self {
        Self {
            bg: None,
            fg: Color::DarkGray,
            ok: Color::Green,
            failed: Color::Red,
            cancelled: Color::DarkGray,
            running: Color::Cyan,
            pending: Color::Yellow,
            skipped: Color::Gray,
            edge: Color::DarkGray,
            title: Color::Gray,
        }
    }
}

/// Color roles (#43), block layout (#49), composer style and feed rhythm
/// (#30) in one copyable bundle. `FeedRenderOptions` carries it so the feed
/// cache fingerprints theme changes; `App` loads it once at startup.
///
/// Palette references are resolved eagerly at parse time — the palette never
/// lives in the runtime theme, so it stays `Copy` and the feed cache keeps
/// comparing plain values.
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
    // ── screen viewport ────────────────────────────────────────────────────
    /// `[screen]` viewport inset: keeps the whole UI clear of the terminal
    /// edges (left/right breathing room especially).
    pub screen: ScreenStyle,
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
    // ── feed rhythm (#30) ──────────────────────────────────────────────────
    pub feed: FeedTheme,
    // ── component style tables (#31) ───────────────────────────────────────
    pub statusbar: StatusbarStyle,
    pub picker: PickerStyle,
    pub sidebar: SidebarStyle,
    pub dag_band: DagBandStyle,
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
            screen: ScreenStyle::default(),
            user: BlockTheme::default(),
            assistant: BlockTheme::default(),
            tool: BlockTheme::default(),
            thinking: BlockTheme::default(),
            composer: ComposerStyle::default(),
            feed: FeedTheme::default(),
            statusbar: StatusbarStyle::default(),
            picker: PickerStyle::default(),
            sidebar: SidebarStyle::default(),
            dag_band: DagBandStyle::default(),
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

    /// Parse the v2 theme.toml (superset of v1). Unknown sections / keys /
    /// roles, invalid hex, unknown align and invalid values warn on stderr
    /// and keep the current value; everything missing stays default.
    pub fn parse(text: &str) -> Self {
        let mut theme = Theme::default();
        let table: TomlTable = match text.parse() {
            Ok(table) => table,
            Err(err) => {
                warn(&format!("parse error: {err} — using defaults"));
                return theme;
            }
        };
        // Palette first: any slot can reference `p:name` regardless of the
        // section order in the file.
        let palette = build_palette(&table);
        for (section, value) in &table {
            let Some(section_table) = value.as_table() else {
                warn(&format!("key {section:?} outside any [section] — ignored"));
                continue;
            };
            match section.as_str() {
                "palette" => {}
                "colors" => apply_color_section(&mut theme, section_table, &palette),
                "screen" => apply_screen_section(&mut theme.screen, section_table),
                "composer" => apply_composer_section(&mut theme.composer, section_table, &palette),
                "blocks" => apply_blocks_section(&mut theme, section_table, &palette),
                "feed" => apply_feed_section(&mut theme.feed, section_table, &palette),
                "statusbar" => apply_style_section(
                    "statusbar",
                    section_table,
                    &palette,
                    &mut [
                        ("fg", &mut theme.statusbar.fg),
                        ("accent", &mut theme.statusbar.accent),
                        ("error", &mut theme.statusbar.error),
                        ("busy", &mut theme.statusbar.busy),
                    ],
                    &mut [("bg", &mut theme.statusbar.bg)],
                ),
                "picker" => apply_style_section(
                    "picker",
                    section_table,
                    &palette,
                    &mut [
                        ("fg", &mut theme.picker.fg),
                        ("highlight_bg", &mut theme.picker.highlight_bg),
                        ("highlight_fg", &mut theme.picker.highlight_fg),
                        ("title", &mut theme.picker.title),
                        ("dim", &mut theme.picker.dim),
                    ],
                    &mut [("bg", &mut theme.picker.bg)],
                ),
                "sidebar" => apply_style_section(
                    "sidebar",
                    section_table,
                    &palette,
                    &mut [
                        ("fg", &mut theme.sidebar.fg),
                        ("heading", &mut theme.sidebar.heading),
                        ("section", &mut theme.sidebar.section),
                        ("badge", &mut theme.sidebar.badge),
                        ("warn", &mut theme.sidebar.warn),
                        ("muted", &mut theme.sidebar.muted),
                    ],
                    &mut [("bg", &mut theme.sidebar.bg)],
                ),
                "dag_band" => apply_style_section(
                    "dag_band",
                    section_table,
                    &palette,
                    &mut [
                        ("fg", &mut theme.dag_band.fg),
                        ("ok", &mut theme.dag_band.ok),
                        ("failed", &mut theme.dag_band.failed),
                        ("cancelled", &mut theme.dag_band.cancelled),
                        ("running", &mut theme.dag_band.running),
                        ("pending", &mut theme.dag_band.pending),
                        ("skipped", &mut theme.dag_band.skipped),
                        ("edge", &mut theme.dag_band.edge),
                        ("title", &mut theme.dag_band.title),
                    ],
                    &mut [("bg", &mut theme.dag_band.bg)],
                ),
                unknown => warn(&format!("unknown section {unknown:?} — ignored")),
            }
        }
        theme
    }
}

/// `${THEWAY_DIR:-$HOME/.theway}/theme.toml`, matching the runtime-state
/// layout documented in AGENTS.md.
fn theme_toml_path() -> PathBuf {
    theway_transport::config::base_dir().join("theme.toml")
}

fn warn(msg: &str) {
    eprintln!("theway theme: {msg}");
}

// ── palette ────────────────────────────────────────────────────────────────

/// Collect the raw `[palette]` table: name → literal string.
fn raw_palette(table: &TomlTable) -> BTreeMap<String, String> {
    let mut raw = BTreeMap::new();
    let Some(toml::Value::Table(palette)) = table.get("palette") else {
        return raw;
    };
    for (name, value) in palette {
        match value.as_str() {
            Some(text) => {
                raw.insert(name.clone(), text.to_string());
            }
            None => warn(&format!(
                "palette.{name}: expected a string color, got {value:?}"
            )),
        }
    }
    raw
}

/// Resolve every palette entry to a concrete color. Entries may reference
/// other entries (`p:other`); cycles and unresolvable references warn once
/// and resolve to `None`.
fn build_palette(table: &TomlTable) -> BTreeMap<String, Option<Color>> {
    let raw = raw_palette(table);
    let mut resolved: BTreeMap<String, Option<Color>> = BTreeMap::new();
    let mut stack: Vec<&str> = Vec::new();
    for name in raw.keys() {
        resolve_palette_entry(name, &raw, &mut resolved, &mut stack);
    }
    resolved
}

fn resolve_palette_entry<'a>(
    name: &'a str,
    raw: &'a BTreeMap<String, String>,
    resolved: &mut BTreeMap<String, Option<Color>>,
    stack: &mut Vec<&'a str>,
) -> Option<Color> {
    if let Some(color) = resolved.get(name) {
        return *color;
    }
    if stack.contains(&name) {
        let cycle = stack.join(" -> ");
        warn(&format!(
            "palette.{name}: reference cycle detected ({cycle}) — ignoring"
        ));
        resolved.insert(name.to_string(), None);
        return None;
    }
    stack.push(name);
    let Some(value) = raw.get(name) else {
        resolved.insert(name.to_string(), None);
        return None;
    };
    let color = match value.strip_prefix("p:") {
        Some(referenced) => resolve_palette_entry(referenced, raw, resolved, stack),
        None => parse_literal_color(value),
    };
    stack.pop();
    resolved.insert(name.to_string(), color);
    color
}

// ── color literals ─────────────────────────────────────────────────────────

/// `#RRGGBB` / `#RGB` / ANSI names / 256-palette index → [`Color`]; anything
/// else → `None` for the caller to warn on.
fn parse_literal_color(value: &str) -> Option<Color> {
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<char> = hex.chars().collect();
        let expanded: String = match digits.len() {
            6 => hex.to_string(),
            3 => digits.iter().flat_map(|c| [*c, *c]).collect(),
            _ => return None,
        };
        if expanded.len() != 6 || !expanded.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return u32::from_str_radix(&expanded, 16).ok().map(Color::from_u32);
    }
    match value.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "darkgray" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "default" => Some(Color::Reset),
        _ => value.parse::<u8>().ok().map(Color::Indexed),
    }
}

/// Resolve a slot value: `p:name` looks up the (already resolved) palette;
/// anything else parses as a literal.
fn resolve_slot_color(value: &str, palette: &BTreeMap<String, Option<Color>>) -> Option<Color> {
    if let Some(name) = value.strip_prefix("p:") {
        return palette.get(name).copied().flatten();
    }
    parse_literal_color(value)
}

fn set_color(slot: &mut Color, key: &str, value: &str, palette: &BTreeMap<String, Option<Color>>) {
    match resolve_slot_color(value, palette) {
        Some(color) => *slot = color,
        None => warn(&format!(
            "{key}: invalid color {value:?} — keeping the current value"
        )),
    }
}

/// Optional slots additionally accept `transparent` / `none` to clear the
/// color (no background).
fn set_opt_color(
    slot: &mut Option<Color>,
    key: &str,
    value: &str,
    palette: &BTreeMap<String, Option<Color>>,
) {
    if matches!(value, "transparent" | "none") {
        *slot = None;
        return;
    }
    match resolve_slot_color(value, palette) {
        Some(color) => *slot = Some(color),
        None => warn(&format!(
            "{key}: invalid color {value:?} — keeping the current value"
        )),
    }
}

/// Parse a block border weight literal (`none | thin | thick`).
fn parse_block_border(value: &str) -> Option<BlockBorder> {
    match value {
        "none" => Some(BlockBorder::None),
        "thin" => Some(BlockBorder::Thin),
        "thick" => Some(BlockBorder::Thick),
        _ => None,
    }
}

/// String value of a toml value, warning when it is not a string.
fn as_str<'a>(key: &str, value: &'a toml::Value) -> Option<&'a str> {
    match value.as_str() {
        Some(text) => Some(text),
        None => {
            warn(&format!("{key}: expected a string value, got {value:?}"));
            None
        }
    }
}

/// Non-negative integer value: accepts a toml integer or a numeric string.
fn as_u16(value: &toml::Value) -> Option<u16> {
    match value {
        toml::Value::Integer(i) => u16::try_from(*i).ok(),
        _ => value.as_str().and_then(|s| s.parse().ok()),
    }
}

// ── section appliers ───────────────────────────────────────────────────────

fn apply_color_section(
    theme: &mut Theme,
    section: &TomlTable,
    palette: &BTreeMap<String, Option<Color>>,
) {
    for (key, value) in section {
        let Some(value) = as_str(key, value) else {
            continue;
        };
        match key.as_str() {
            "user_text" => set_color(
                &mut theme.user_text,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "user_bg" => set_color(&mut theme.user_bg, &format!("colors.{key}"), value, palette),
            "assistant_text" => set_opt_color(
                &mut theme.assistant_text,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "assistant_prefix" => set_color(
                &mut theme.assistant_prefix,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_title" => set_color(
                &mut theme.tool_title,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_args" => set_color(
                &mut theme.tool_args,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_result" => set_color(
                &mut theme.tool_result,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_error" => set_color(
                &mut theme.tool_error,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_running_bg" => set_opt_color(
                &mut theme.tool_running_bg,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_success_bg" => set_opt_color(
                &mut theme.tool_success_bg,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "tool_error_bg" => set_opt_color(
                &mut theme.tool_error_bg,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "thinking_text" => set_color(
                &mut theme.thinking_text,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            "thinking_bg" => set_opt_color(
                &mut theme.thinking_bg,
                &format!("colors.{key}"),
                value,
                palette,
            ),
            unknown => warn(&format!("colors.{unknown}: unknown role — ignored")),
        }
    }
}

fn apply_blocks_section(
    theme: &mut Theme,
    section: &TomlTable,
    palette: &BTreeMap<String, Option<Color>>,
) {
    for (name, value) in section {
        let Some(block_table) = value.as_table() else {
            warn(&format!("blocks.{name}: expected a table — ignored"));
            continue;
        };
        let block = match name.as_str() {
            "user" => &mut theme.user,
            "assistant" => &mut theme.assistant,
            "tool" => &mut theme.tool,
            "thinking" => &mut theme.thinking,
            unknown => {
                warn(&format!("blocks.{unknown}: unknown block kind — ignored"));
                continue;
            }
        };
        for (key, value) in block_table {
            match key.as_str() {
                "padding" => match as_u16(value) {
                    Some(padding) => block.padding = padding,
                    None => warn(&format!(
                        "blocks.{name}.padding: invalid padding {value:?} — keeping the current value"
                    )),
                },
                "margin_top" => match as_u16(value) {
                    Some(margin) => block.margin_top = margin,
                    None => warn(&format!(
                        "blocks.{name}.margin_top: invalid margin {value:?} — keeping the current value"
                    )),
                },
                "margin_bottom" => match as_u16(value) {
                    Some(margin) => block.margin_bottom = margin,
                    None => warn(&format!(
                        "blocks.{name}.margin_bottom: invalid margin {value:?} — keeping the current value"
                    )),
                },
                _ => {
                    let Some(value) = as_str(&format!("blocks.{name}.{key}"), value) else {
                        continue;
                    };
                    match key.as_str() {
                        "bg" => match resolve_slot_color(value, palette) {
                            Some(color) => block.bg = Some(color),
                            None => warn(&format!(
                                "blocks.{name}.bg: invalid hex {value:?} — keeping the current value"
                            )),
                        },
                        "align" => match value {
                            "left" => block.align = BlockAlign::Left,
                            "right" => block.align = BlockAlign::Right,
                            other => warn(&format!(
                                "blocks.{name}.align: unknown alignment {other:?} — keeping the current value"
                            )),
                        },
                        "border_top" => match parse_block_border(value) {
                            Some(border) => block.border_top = border,
                            None => warn(&format!(
                                "blocks.{name}.border_top: unknown border {value:?} (none|thin|thick) — keeping the current value"
                            )),
                        },
                        "border_bottom" => match parse_block_border(value) {
                            Some(border) => block.border_bottom = border,
                            None => warn(&format!(
                                "blocks.{name}.border_bottom: unknown border {value:?} (none|thin|thick) — keeping the current value"
                            )),
                        },
                        "border_style" => match resolve_slot_color(value, palette) {
                            Some(color) => block.border_style = color,
                            None => warn(&format!(
                                "blocks.{name}.border_style: invalid color {value:?} — keeping the current value"
                            )),
                        },
                        unknown => warn(&format!("blocks.{name}.{unknown}: unknown key — ignored")),
                    }
                }
            }
        }
    }
}

/// `[screen]` viewport inset: `margin` (uniform, all four sides) plus
/// per-side `margin_top/right/bottom/left` overrides.
fn apply_screen_section(screen: &mut ScreenStyle, section: &TomlTable) {
    for (key, value) in section {
        match key.as_str() {
            "margin" => match as_u16(value) {
                Some(margin) => {
                    screen.margin_top = margin;
                    screen.margin_right = margin;
                    screen.margin_bottom = margin;
                    screen.margin_left = margin;
                }
                None => warn(&format!(
                    "screen.margin: invalid margin {value:?} — keeping the current value"
                )),
            },
            "margin_top" => match as_u16(value) {
                Some(margin) => screen.margin_top = margin,
                None => warn(&format!(
                    "screen.margin_top: invalid margin {value:?} — keeping the current value"
                )),
            },
            "margin_right" => match as_u16(value) {
                Some(margin) => screen.margin_right = margin,
                None => warn(&format!(
                    "screen.margin_right: invalid margin {value:?} — keeping the current value"
                )),
            },
            "margin_bottom" => match as_u16(value) {
                Some(margin) => screen.margin_bottom = margin,
                None => warn(&format!(
                    "screen.margin_bottom: invalid margin {value:?} — keeping the current value"
                )),
            },
            "margin_left" => match as_u16(value) {
                Some(margin) => screen.margin_left = margin,
                None => warn(&format!(
                    "screen.margin_left: invalid margin {value:?} — keeping the current value"
                )),
            },
            unknown => warn(&format!("screen.{unknown}: unknown key — ignored")),
        }
    }
}

fn apply_feed_section(
    feed: &mut FeedTheme,
    section: &TomlTable,
    palette: &BTreeMap<String, Option<Color>>,
) {
    for (key, value) in section {
        match key.as_str() {
            "gap" => match as_u16(value) {
                Some(gap) => feed.gap = gap,
                None => warn(&format!(
                    "feed.gap: invalid gap {value:?} — keeping the current value"
                )),
            },
            _ => {
                let Some(value) = as_str(&format!("feed.{key}"), value) else {
                    continue;
                };
                match key.as_str() {
                    "separator" => match value.chars().count() {
                        0 => feed.separator = None,
                        1 => feed.separator = value.chars().next(),
                        _ => warn(&format!(
                            "feed.separator: expected a single glyph, got {value:?} — keeping the current value"
                        )),
                    },
                    "separator_style" => set_color(
                        &mut feed.separator_style,
                        "feed.separator_style",
                        value,
                        palette,
                    ),
                    unknown => warn(&format!("feed.{unknown}: unknown key — ignored")),
                }
            }
        }
    }
}

fn apply_composer_section(
    composer: &mut ComposerStyle,
    section: &TomlTable,
    palette: &BTreeMap<String, Option<Color>>,
) {
    for (key, value) in section {
        let Some(value) = as_str(&format!("composer.{key}"), value) else {
            continue;
        };
        let slot = match key.as_str() {
            "border_focused" => Some(&mut composer.border_focused),
            "border_unfocused" => Some(&mut composer.border_unfocused),
            "prefix" => Some(&mut composer.prefix),
            "text" => Some(&mut composer.text),
            "bg" => Some(&mut composer.bg),
            "info_text" => Some(&mut composer.info_text),
            "placeholder" => Some(&mut composer.placeholder),
            "hint" => Some(&mut composer.hint),
            "cursor" => Some(&mut composer.cursor),
            unknown => {
                warn(&format!("composer.{unknown}: unknown key — ignored"));
                None
            }
        };
        let Some(slot) = slot else { continue };
        set_color(slot, &format!("composer.{key}"), value, palette);
    }
}

/// Generic applier for the flat component style tables (`[statusbar]` /
/// `[picker]` / `[sidebar]` / `[dag_band]`, issue #31): every key is a color
/// slot; `opt_slots` additionally accept `transparent`/`none` to clear.
fn apply_style_section(
    label: &str,
    section: &TomlTable,
    palette: &BTreeMap<String, Option<Color>>,
    color_slots: &mut [(&str, &mut Color)],
    opt_slots: &mut [(&str, &mut Option<Color>)],
) {
    for (key, value) in section {
        let Some(value) = as_str(&format!("{label}.{key}"), value) else {
            continue;
        };
        if let Some(slot) = opt_slots.iter_mut().find(|(name, _)| *name == key) {
            set_opt_color(slot.1, &format!("{label}.{key}"), value, palette);
            continue;
        }
        if let Some(slot) = color_slots.iter_mut().find(|(name, _)| *name == key) {
            set_color(slot.1, &format!("{label}.{key}"), value, palette);
            continue;
        }
        warn(&format!("{label}.{key}: unknown key — ignored"));
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
        // v2 feed rhythm defaults: one blank line, no separator glyph.
        assert_eq!(t.feed, FeedTheme::default());
        assert_eq!(t.feed.gap, 1);
        assert_eq!(t.feed.separator, None);
        assert_eq!(t.feed.separator_style, prompt_chrome::GRAY_DIM);
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
        assert_eq!(theme.feed, d.feed);
    }

    #[test]
    fn parse_unknown_role_section_and_key_fall_back() {
        let theme = Theme::parse(
            "[colors]\nuser_text = \"#010203\"\nwat = \"#ffffff\"\n\
             [blocks.foo]\nbg = \"#999999\"\n\
             [blocks.tool]\nwobble = 3\n\
             [composer]\nwut = \"#888888\"\n\
             [feed]\nwibble = 2\n",
        );
        let d = Theme::default();
        assert_eq!(theme.user_text, Color::Rgb(1, 2, 3));
        assert_eq!(theme.tool_title, d.tool_title);
        assert_eq!(theme.tool.bg, d.tool.bg);
        assert_eq!(theme.tool.padding, d.tool.padding);
        assert_eq!(theme.composer, d.composer);
        assert_eq!(theme.feed, d.feed);
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
        assert_eq!(theme.feed, d.feed);
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
    fn parse_toml_syntax_error_uses_defaults() {
        let theme = Theme::parse("[colors\nuser_text = \"#010203\"\n");
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

    // ── v2: feed rhythm (#30) ────────────────────────────────────────────

    #[test]
    fn screen_margin_defaults_to_flush() {
        let theme = Theme::default();
        assert_eq!(
            theme.screen,
            ScreenStyle {
                margin_top: 0,
                margin_right: 0,
                margin_bottom: 0,
                margin_left: 0,
            }
        );
        // Flush inset is a no-op on the viewport.
        let rect = Rect::new(0, 0, 80, 24);
        assert_eq!(theme.screen.inset(rect), rect);
    }

    #[test]
    fn screen_margin_parses_uniform_and_per_side() {
        let theme = Theme::parse("[screen]\nmargin = 2\n");
        assert_eq!(
            theme.screen,
            ScreenStyle {
                margin_top: 2,
                margin_right: 2,
                margin_bottom: 2,
                margin_left: 2,
            }
        );

        // Per-side keys override the uniform margin.
        let theme = Theme::parse("[screen]\nmargin = 2\nmargin_left = 4\nmargin_top = 0\n");
        assert_eq!(
            theme.screen,
            ScreenStyle {
                margin_top: 0,
                margin_right: 2,
                margin_bottom: 2,
                margin_left: 4,
            }
        );

        // Invalid / unknown values keep defaults and warn.
        let theme = Theme::parse("[screen]\nmargin = -1\nmargin_left = \"wide\"\nfoo = 1\n");
        assert_eq!(theme.screen, ScreenStyle::default());
    }

    #[test]
    fn screen_margin_inset_is_saturating() {
        let screen = ScreenStyle {
            margin_top: 3,
            margin_right: 100,
            margin_bottom: 3,
            margin_left: 4,
        };
        // Width collapses to zero rather than underflowing.
        let rect = screen.inset(Rect::new(0, 0, 80, 24));
        assert_eq!(rect.x, 4);
        assert_eq!(rect.y, 3);
        assert_eq!(rect.width, 0);
        assert_eq!(rect.height, 18);
    }

    #[test]
    fn feed_gap_parses_and_defaults() {
        let theme = Theme::parse("[feed]\ngap = 3\n");
        assert_eq!(theme.feed.gap, 3);

        let theme = Theme::parse("[feed]\ngap = 0\n");
        assert_eq!(theme.feed.gap, 0);

        // Negative / non-numeric gaps fall back to the default.
        let theme = Theme::parse("[feed]\ngap = -1\n");
        assert_eq!(theme.feed.gap, 1);
        let theme = Theme::parse("[feed]\ngap = \"lots\"\n");
        assert_eq!(theme.feed.gap, 1);
    }

    #[test]
    fn feed_separator_parses_glyph_style_and_empty() {
        let theme = Theme::parse("[feed]\nseparator = \"─\"\nseparator_style = \"#565F89\"\n");
        assert_eq!(theme.feed.separator, Some('─'));
        assert_eq!(theme.feed.separator_style, Color::Rgb(0x56, 0x5F, 0x89));

        // Empty string clears the glyph; multi-char glyphs are rejected.
        let theme = Theme::parse("[feed]\nseparator = \"\"\n");
        assert_eq!(theme.feed.separator, None);
        let theme = Theme::parse("[feed]\nseparator = \"──\"\n");
        assert_eq!(theme.feed.separator, None);
    }

    // ── v2: palette + extended literals (#30) ────────────────────────────

    #[test]
    fn palette_references_resolve_across_sections() {
        let theme = Theme::parse(
            "[palette]\naccent = \"#7AA2F7\"\nmuted = \"p:accent\"\n\
             [colors]\nuser_text = \"p:accent\"\nthinking_bg = \"p:muted\"\n\
             [blocks.tool]\nbg = \"p:accent\"\n\
             [composer]\nprefix = \"p:accent\"\n\
             [feed]\nseparator_style = \"p:muted\"\n",
        );
        let accent = Color::Rgb(0x7A, 0xA2, 0xF7);
        assert_eq!(theme.user_text, accent);
        assert_eq!(theme.thinking_bg, Some(accent));
        assert_eq!(theme.tool.bg, Some(accent));
        assert_eq!(theme.composer.prefix, accent);
        assert_eq!(theme.feed.separator_style, accent);
    }

    #[test]
    fn palette_missing_key_and_cycle_fall_back() {
        // Unknown palette reference → warn + keep the slot default.
        let theme = Theme::parse("[colors]\nuser_text = \"p:nope\"\n");
        assert_eq!(theme.user_text, Theme::default().user_text);

        // Cyclic palette entries resolve to nothing.
        let theme =
            Theme::parse("[palette]\na = \"p:b\"\nb = \"p:a\"\n[colors]\nuser_text = \"p:a\"\n");
        assert_eq!(theme.user_text, Theme::default().user_text);

        // A palette entry referencing a literal resolves.
        let theme = Theme::parse("[palette]\na = \"#010203\"\n[colors]\nuser_text = \"p:a\"\n");
        assert_eq!(theme.user_text, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn transparent_and_none_clear_optional_slots() {
        // Set first, then clear via transparent / none.
        let theme = Theme::parse(
            "[colors]\nthinking_bg = \"#0d0e0f\"\n\
             [blocks.tool]\nbg = \"#111213\"\n",
        );
        assert_eq!(theme.thinking_bg, Some(Color::Rgb(13, 14, 15)));
        assert_eq!(theme.tool.bg, Some(Color::Rgb(17, 18, 19)));

        let theme = Theme::parse(
            "[colors]\nthinking_bg = \"transparent\"\n\
             [blocks.tool]\nbg = \"none\"\n",
        );
        assert_eq!(theme.thinking_bg, None);
        assert_eq!(theme.tool.bg, None);

        // Required slots reject transparent (warn + keep).
        let theme = Theme::parse("[composer]\nbg = \"transparent\"\n");
        assert_eq!(theme.composer.bg, Theme::default().composer.bg);
    }

    #[test]
    fn extended_color_literals_parse() {
        let theme = Theme::parse(
            "[colors]\nuser_text = \"#7AF\"\nassistant_prefix = \"red\"\n\
             tool_title = \"146\"\nthinking_text = \"default\"\n\
             user_bg = \"lightBlue\"\n",
        );
        assert_eq!(theme.user_text, Color::Rgb(0x77, 0xAA, 0xFF));
        assert_eq!(theme.assistant_prefix, Color::Red);
        assert_eq!(theme.tool_title, Color::Indexed(146));
        assert_eq!(theme.thinking_text, Color::Reset);
        assert_eq!(theme.user_bg, Color::LightBlue);
    }
}

// ── v2 phase 2 (#31): block margins/borders + component tables ───────

#[test]
fn default_block_and_component_tables_match_hardcoded() {
    let d = Theme::default();
    for block in [d.user, d.assistant, d.tool, d.thinking] {
        assert_eq!(block.margin_top, 0);
        assert_eq!(block.margin_bottom, 0);
        assert_eq!(block.border_top, BlockBorder::None);
        assert_eq!(block.border_bottom, BlockBorder::None);
        assert_eq!(block.border_style, crate::ui::prompt_chrome::GRAY_DIM);
    }
    assert_eq!(d.composer.placeholder, crate::ui::prompt_chrome::GRAY);
    assert_eq!(d.composer.hint, Color::DarkGray);
    assert_eq!(d.composer.cursor, crate::ui::prompt_chrome::TEXT_PRIMARY);
    assert_eq!(d.statusbar.fg, Color::DarkGray);
    assert_eq!(d.statusbar.accent, Color::Yellow);
    assert_eq!(d.statusbar.busy, Color::Gray);
    assert_eq!(d.picker.fg, Color::Cyan);
    assert_eq!(d.picker.highlight_bg, Color::Cyan);
    assert_eq!(d.picker.highlight_fg, Color::Black);
    assert_eq!(d.picker.title, Color::Yellow);
    assert_eq!(d.sidebar.fg, Color::DarkGray);
    assert_eq!(d.sidebar.heading, Color::Magenta);
    assert_eq!(d.sidebar.section, Color::Cyan);
    assert_eq!(d.dag_band.ok, Color::Green);
    assert_eq!(d.dag_band.failed, Color::Red);
    assert_eq!(d.dag_band.cancelled, Color::DarkGray);
    assert_eq!(d.dag_band.running, Color::Cyan);
    assert_eq!(d.dag_band.pending, Color::Yellow);
    assert_eq!(d.dag_band.skipped, Color::Gray);
    assert_eq!(d.dag_band.title, Color::Gray);
}

#[test]
fn parse_block_margins_and_borders() {
    let theme = Theme::parse(
        "[blocks.tool]\nmargin_top = 1\nmargin_bottom = 2\n\
             border_top = \"thin\"\nborder_bottom = \"thick\"\nborder_style = \"#010203\"\n",
    );
    assert_eq!(theme.tool.margin_top, 1);
    assert_eq!(theme.tool.margin_bottom, 2);
    assert_eq!(theme.tool.border_top, BlockBorder::Thin);
    assert_eq!(theme.tool.border_bottom, BlockBorder::Thick);
    assert_eq!(theme.tool.border_style, Color::Rgb(1, 2, 3));
    // Unset blocks keep defaults.
    assert_eq!(theme.thinking.margin_top, 0);
    assert_eq!(theme.thinking.border_top, BlockBorder::None);

    // Invalid border literal falls back with a warning.
    let theme = Theme::parse("[blocks.tool]\nborder_top = \"dashed\"\n");
    assert_eq!(theme.tool.border_top, BlockBorder::None);
    // Negative margins fall back.
    let theme = Theme::parse("[blocks.tool]\nmargin_top = -3\n");
    assert_eq!(theme.tool.margin_top, 0);
}

#[test]
fn parse_component_style_tables() {
    let theme = Theme::parse(
        "[composer]\nplaceholder = \"#111111\"\nhint = \"#222222\"\ncursor = \"#333333\"\n\
             [statusbar]\nfg = \"#444444\"\nbg = \"#555555\"\n\
             [picker]\ntitle = \"#666666\"\nbg = \"transparent\"\n\
             [sidebar]\nheading = \"#777777\"\n\
             [dag_band]\nok = \"#888888\"\nfailed = \"p:accent\"\n\
             [palette]\naccent = \"#999999\"\n",
    );
    assert_eq!(theme.composer.placeholder, Color::Rgb(0x11, 0x11, 0x11));
    assert_eq!(theme.composer.hint, Color::Rgb(0x22, 0x22, 0x22));
    assert_eq!(theme.composer.cursor, Color::Rgb(0x33, 0x33, 0x33));
    assert_eq!(theme.statusbar.fg, Color::Rgb(0x44, 0x44, 0x44));
    assert_eq!(theme.statusbar.bg, Some(Color::Rgb(0x55, 0x55, 0x55)));
    assert_eq!(theme.picker.title, Color::Rgb(0x66, 0x66, 0x66));
    assert_eq!(theme.picker.bg, None);
    assert_eq!(theme.sidebar.heading, Color::Rgb(0x77, 0x77, 0x77));
    assert_eq!(theme.dag_band.ok, Color::Rgb(0x88, 0x88, 0x88));
    assert_eq!(theme.dag_band.failed, Color::Rgb(0x99, 0x99, 0x99));
    // Unknown keys in the new sections warn + keep.
    let d = Theme::default();
    assert_eq!(theme.statusbar.accent, d.statusbar.accent);
}
