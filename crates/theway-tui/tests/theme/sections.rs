use super::*;

    #[test]
    fn parse_feed_section_applies_gap_separator_and_separate_all() {
        let theme = Theme::parse(
            r##"
[feed]
gap = 3
separator = "─"
separate_all = true
"##,
        );
        assert_eq!(theme.feed.gap, 3);
        assert_eq!(theme.feed.separator, Some('─'));
        assert!(theme.feed.separate_all);

        // Defaults when unset.
        let theme = Theme::parse("");
        assert_eq!(theme.feed.gap, 1);
        assert_eq!(theme.feed.separator, None);
        assert!(!theme.feed.separate_all);

        // Invalid values keep the current value.
        let theme = Theme::parse("[feed]\nseparate_all = \"yes\"\n");
        assert!(!theme.feed.separate_all);
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

