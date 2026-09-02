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
                    &mut [("stats_format", &mut theme.statusbar.stats_format)],
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
                    &mut [],
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
                    &mut [],
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
                    &mut [],
                ),
                unknown => warn(&format!("unknown section {unknown:?} — ignored")),
            }
        }
        theme
    }
}
