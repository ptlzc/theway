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
    /// When set, EVERY adjacent block pair gets a gap (tool→tool,
    /// assistant→tool, …), not just the user-message boundaries.
    pub separate_all: bool,
}

impl Default for FeedTheme {
    fn default() -> Self {
        Self {
            gap: 1,
            separator: None,
            separator_style: GRAY_DIM,
            separate_all: false,
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
    /// Prompt background surface. Kept for theme-file compatibility only —
    /// the composer renders transparent since the background was removed;
    /// setting this key has no visual effect.
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
/// the terminal edges. `margin = N` sets all four sides; per-side
/// `margin_top/right/bottom/left` keys override individual sides. The
/// default left margin is 2 columns (the UI sits off the terminal's left
/// edge); the other sides stay flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenStyle {
    pub margin_top: u16,
    pub margin_right: u16,
    pub margin_bottom: u16,
    pub margin_left: u16,
}

impl Default for ScreenStyle {
    fn default() -> Self {
        Self {
            margin_top: 0,
            margin_right: 0,
            margin_bottom: 0,
            margin_left: 2,
        }
    }
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
    /// Optional template for the busy-band stats line. Supports `{tps}`,
    /// `{in}`, `{out}`, and `{hit}` placeholders. `None` uses the built-in
    /// default (`{tps} t/s · in: {in} · out: {out} · cache {hit}`).
    pub stats_format: Option<&'static str>,
}

impl Default for StatusbarStyle {
    fn default() -> Self {
        Self {
            bg: None,
            fg: Color::DarkGray,
            accent: Color::Yellow,
            error: Color::Red,
            busy: Color::Gray,
            stats_format: None,
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
