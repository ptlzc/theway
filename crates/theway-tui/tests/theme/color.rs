use super::*;

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

