use super::*;

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
    fn screen_margin_defaults_to_left_2() {
        let theme = Theme::default();
        assert_eq!(
            theme.screen,
            ScreenStyle {
                margin_top: 0,
                margin_right: 0,
                margin_bottom: 0,
                margin_left: 2,
            }
        );
        // The default left inset shifts the viewport by 2 columns.
        let rect = Rect::new(0, 0, 80, 24);
        assert_eq!(theme.screen.inset(rect), Rect::new(2, 0, 78, 24));
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

