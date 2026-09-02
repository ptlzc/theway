/// Grok tokyonight palette values (xai-grok-pager-render theme/tokyonight.rs).
const ACCENT_USER: Color = Color::Rgb(122, 162, 247); // BLUE — user `❯` prefix
const ACCENT_ASSISTANT: Color = Color::Rgb(187, 154, 247); // legacy theme role
const ACCENT_TOOL: Color = Color::Rgb(115, 122, 162); // DARK5 — tool name
const TEXT_PRIMARY: Color = Color::Rgb(192, 202, 245); // FG — body text
const TEXT_SECONDARY: Color = Color::Rgb(169, 177, 214); // TEXT_SECONDARY — secondary text
const BG_HIGHLIGHT: Color = Color::Rgb(41, 46, 66); // BG_HIGHLIGHT — user band / selection

// ── Theme-role defaults (issue #43) ─────────────────────────────────────────
// The pre-theme hardcoded colors, kept as the single source of truth for
// `Theme::default()`: a build without `~/.theway/theme.toml` renders exactly
// as before.
pub(crate) const USER_TEXT_DEFAULT: Color = TEXT_PRIMARY;
pub(crate) const USER_BG_DEFAULT: Color = BG_HIGHLIGHT;
pub(crate) const ASSISTANT_TEXT_DEFAULT: Option<Color> = None;
pub(crate) const ASSISTANT_PREFIX_DEFAULT: Color = ACCENT_ASSISTANT;
pub(crate) const TOOL_TITLE_DEFAULT: Color = ACCENT_TOOL;
pub(crate) const TOOL_ARGS_DEFAULT: Color = Color::DarkGray;
pub(crate) const TOOL_RESULT_DEFAULT: Color = TEXT_SECONDARY; // neutral gray, not green
pub(crate) const TOOL_ERROR_DEFAULT: Color = Color::Red;
pub(crate) const TOOL_RUNNING_BG_DEFAULT: Option<Color> = None;
pub(crate) const TOOL_SUCCESS_BG_DEFAULT: Option<Color> = None;
pub(crate) const TOOL_ERROR_BG_DEFAULT: Option<Color> = None;
pub(crate) const THINKING_TEXT_DEFAULT: Color = Color::DarkGray;
pub(crate) const THINKING_BG_DEFAULT: Option<Color> = None;

pub(crate) const USER_PREFIX: &str = "\u{276F} "; // ❯ (2 cols, grok prompt_arrow)
pub(crate) const TOOL_PREFIX: &str = "\u{23f5} "; // ⏵
const USER_BAND_INDENT: &str = "  ";

const USER_STYLE: Style = Style::new().fg(ACCENT_USER).add_modifier(Modifier::BOLD);
/// Default thinking style; the streaming thinking path in `feed_cache`
/// reuses this const (theme-aware colors apply to one-shot renders).
pub(crate) const THINKING_STYLE: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
pub(crate) const RESULT_SUMMARY_STYLE: Style = Style::new().fg(Color::DarkGray);

fn user_body_style(theme: &Theme, bg: Color) -> Style {
    Style::new().fg(theme.user_text).bg(bg)
}
fn tool_name_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.tool_title)
        .add_modifier(Modifier::BOLD)
}
fn tool_args_style(theme: &Theme) -> Style {
    Style::new().fg(theme.tool_args)
}
fn thinking_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.thinking_text)
        .add_modifier(Modifier::ITALIC)
}

/// How `Block::Thinking` renders in the feed (Ctrl+O cycles).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThinkingMode {
    /// Full thinking text (default).
    #[default]
    Full,
    /// Peek window: header + the last few lines only.
    Peek,
    /// Skipped entirely.
    Hidden,
}

/// Renderer switches owned by the TUI app state.
///
/// `PartialEq` is hand-implemented over the structural switches only
/// (`thinking_mode` / `tools_expanded` / `theme`): the per-frame counters
/// (`thinking_cps` / `thinking_input_tokens` / `thinking_output_tokens` /
/// `spinner_phase`) change every frame while a turn streams and must NOT
/// participate in equality — otherwise the feed cache sees a new option set
/// every frame and the #34/#35 incremental rendering degrades to full
/// re-renders. Streaming tails re-render their stats line with fresh
/// counters each frame; frozen historical blocks keep the values they were
/// rendered with. The theme is structural: it changes only at startup (or
/// on reload), so any theme change invalidates the whole cache.
#[derive(Clone, Copy, Debug, Default)]
pub struct FeedRenderOptions {
    pub thinking_mode: ThinkingMode,
    /// Tool results: collapsed to a bordered preview unless expanded (Ctrl+T).
    pub tools_expanded: bool,
    /// Terminal capability resolved by the owning client. Tests use the
    /// `TrueColor` default instead of ambient process environment state.
    pub color_level: theway_markdown::ColorLevel,
    /// Thinking-block throughput (chars/sec over the last 1s window) shown on
    /// the stats line; sourced by the CpsMeter (node 3-spinner).
    pub thinking_cps: f64,
    /// Last-turn input token count shown on the thinking stats line.
    pub thinking_input_tokens: u64,
    /// Last-turn output token count shown on the thinking stats line.
    pub thinking_output_tokens: u64,
    /// Rainbow spinner animation phase (node 3-spinner); passthrough, not
    /// consumed by block rendering. Dead until a consumer wires it — kept in
    /// the option set so per-frame animation state travels with the render
    /// switches (and excluded from `PartialEq` like the other per-frame
    /// counters).
    #[allow(dead_code)]
    pub spinner_phase: u32,
    /// Theme color roles + block layout + composer style (issues #43 + #49),
    /// loaded once at startup into `App.theme` and threaded into every
    /// render so the feed cache fingerprints theme changes.
    pub theme: Theme,
}

/// Structural equality only (issue #44 + #49): per-frame counters
/// (cps / in / out / spinner_phase) are excluded so the feed cache keeps its
/// incremental rendering across frames; the theme participates because it
/// changes colors and layout.
impl PartialEq for FeedRenderOptions {
    fn eq(&self, other: &Self) -> bool {
        self.thinking_mode == other.thinking_mode
            && self.tools_expanded == other.tools_expanded
            && self.color_level == other.color_level
            && self.theme == other.theme
    }
}

/// Lines shown in the thinking peek window.
pub(crate) const THINKING_PEEK_LINES: usize = 3;

/// Left border + indent prefixed to each tool result preview line. The bar
/// sits at the block's content edge (no leading indent) so the result body
/// hugs the left side.
const TOOL_RESULT_BORDER: &str = "\u{2502} ";
/// Tool result preview height before the `…(N more lines)` elision row.
const TOOL_RESULT_PREVIEW_LINES: usize = 5;

pub fn style_for_level(level: Level) -> Style {
    match level {
        Level::Output => Style::default(),
        Level::System => Style::default().fg(Color::DarkGray),
        Level::Error => Style::default().fg(Color::Red),
        Level::Note => Style::default().fg(Color::Green),
        Level::Header => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        Level::Qr => Style::default(),
    }
}
