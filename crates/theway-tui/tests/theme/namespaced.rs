use super::*;

    #[test]
    fn theme_namespaced_subtables_parse_like_flat_sections() {
        // A `[theme.*]` subtable (config.toml shape) produces the SAME Theme
        // as the equivalent flat top-level sections.
        let flat = Theme::parse(
            "[screen]\nmargin_left = 4\n\
             [feed]\ngap = 3\nseparator = \"─\"\n\
             [blocks.tool]\nbg = \"#010203\"\npadding = 0\nalign = \"right\"\n",
        );
        let namespaced = Theme::parse_namespaced(
            "[theme.screen]\nmargin_left = 4\n\
             [theme.feed]\ngap = 3\nseparator = \"─\"\n\
             [theme.blocks.tool]\nbg = \"#010203\"\npadding = 0\nalign = \"right\"\n",
        );
        // One representative field per section.
        assert_eq!(namespaced.screen.margin_left, flat.screen.margin_left);
        assert_eq!(namespaced.feed.gap, flat.feed.gap);
        assert_eq!(namespaced.feed.separator, flat.feed.separator);
        assert_eq!(namespaced.tool.bg, flat.tool.bg);
        assert_eq!(namespaced.tool.padding, flat.tool.padding);
        assert_eq!(namespaced.tool.align, flat.tool.align);
    }

    #[test]
    fn parse_namespaced_ignores_non_theme_sections() {
        // A realistic config.toml carries `[model]`, `[ui]` and `[[server]]`
        // tables alongside `[theme.screen]`; the non-theme sections must be
        // ignored and have no effect on the parsed theme.
        let text = r##"
[model]
name = "claude"

[ui]
log_level = "debug"

[[server]]
base_url = "https://example.com"

[theme.screen]
margin_left = 6

[theme.feed]
gap = 2
"##;
        let theme = Theme::parse_namespaced(text);
        assert_eq!(theme.screen.margin_left, 6);
        assert_eq!(theme.feed.gap, 2);
        // The stray top-level sections neither set nor corrupt theme fields.
        let d = Theme::default();
        assert_eq!(theme.user_text, d.user_text);
        assert_eq!(theme.statusbar, d.statusbar);
    }

    #[test]
    fn parse_namespaced_falls_back_to_flat_sections_without_theme_table() {
        // No `[theme]` table present → parse the file flat, so a legacy
        // theme.toml shape still works through this entry point.
        let theme = Theme::parse_namespaced("[screen]\nmargin_left = 8\n[feed]\ngap = 4\n");
        assert_eq!(theme.screen.margin_left, 8);
        assert_eq!(theme.feed.gap, 4);
    }

    #[test]
    fn load_from_config_text_parses_theme_namespace() {
        // The controller reload entry point reads `[theme.*]` out of a
        // config.toml-shaped blob.
        let theme = Theme::load_from_config_text(
            "[theme.blocks.tool]\nbg = \"#010203\"\npadding = 0\n[theme.feed]\ngap = 5\n",
        );
        assert_eq!(theme.tool.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(theme.tool.padding, 0);
        assert_eq!(theme.feed.gap, 5);
    }
