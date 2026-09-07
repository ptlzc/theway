use super::*;

#[test]
fn theme_parse_covers_all_color_names_and_section_appliers() {
    let names = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "gray",
        "darkgray",
        "lightred",
        "lightgreen",
        "lightyellow",
        "lightblue",
        "lightmagenta",
        "lightcyan",
        "default",
        "146",
    ];
    for (i, name) in names.iter().enumerate() {
        let theme = Theme::parse(&format!(
            "[colors]\nuser_text = \"{name}\"\nuser_bg = \"p:missing\"\n\
                 [blocks.tool]\nbg = \"p:nope\"\nalign = \"diagonal\"\nborder_top = \"bad\"\n\
                 [screen]\nmargin = \"bad\"\nmargin_left = \"bad\"\n\
                 [feed]\ngap = \"bad\"\nseparate_all = \"bad\"\nseparator = \"too long\"\n\
                 [composer]\nborder_focused = \"bad\"\nunknown = \"bad\"\n\
                 [statusbar]\nunknown = \"bad\"\n[unknown]\nx = \"y\"\nkey = 1\n"
        ));
        assert_ne!(theme.user_text, Theme::default().user_text, "{name}");
        assert_eq!(theme.user_bg, Theme::default().user_bg);
        assert_eq!(theme.tool.bg, Theme::default().tool.bg);
        assert_eq!(theme.tool.align, Theme::default().tool.align);
        assert_eq!(theme.feed.separator, None);
        assert_eq!(
            theme.composer.border_focused,
            Theme::default().composer.border_focused
        );
        let _ = i;
    }
    // Palette cycle and missing-key warnings.
    let theme =
        Theme::parse("[palette]\na = \"p:b\"\nb = \"p:a\"\n[colors]\nuser_text = \"p:a\"\n");
    assert_eq!(theme.user_text, Theme::default().user_text);
    // Scalar outside a section is ignored.
    assert_eq!(Theme::parse("foo = 1"), Theme::default());
}

#[test]
fn theme_parse_covers_all_section_keys() {
    let theme = Theme::parse(
        r##"
[palette]
accent = "#ff0000"

[colors]
user_text = "p:accent"
user_bg = "#010203"
assistant_text = "transparent"
assistant_prefix = "#040506"
tool_title = "#070809"
tool_args = "#0a0b0c"
tool_result = "#0d0e0f"
tool_error = "#101112"
tool_running_bg = "none"
tool_success_bg = "transparent"
tool_error_bg = "#131415"
thinking_text = "#161718"
thinking_bg = "#191a1b"

[blocks.user]
padding = 1
margin_top = 1
margin_bottom = 2
bg = "#1c1d1e"
align = "right"
border_top = "thin"
border_bottom = "thick"
border_style = "#1f2021"
unknown = "ignored"

[blocks.assistant]
padding = 2
margin_top = 0
margin_bottom = 0
bg = "transparent"
align = "left"
border_top = "none"
border_bottom = "none"
border_style = "#222324"

[blocks.tool]
padding = 3
margin_top = 1
margin_bottom = 1
bg = "#252627"
align = "left"
border_top = "thick"
border_bottom = "thin"
border_style = "#28292a"

[blocks.thinking]
padding = 4
margin_top = 2
margin_bottom = 2
bg = "#2b2c2d"
align = "right"
border_top = "thin"
border_bottom = "none"
border_style = "#2e2f30"

[screen]
margin = 5
margin_top = 1
margin_right = 2
margin_bottom = 3
margin_left = 4

[composer]
border_focused = "#313233"
border_unfocused = "#343536"
prefix = "#373839"
text = "#3a3b3c"
bg = "#3d3e3f"
info_text = "#404142"
placeholder = "#434445"
hint = "#464748"
cursor = "#494a4b"

[thinking]
stats_format = "x {cps}"

[feed]
gap = 2
separate_all = true
separator = "·"
separator_style = "#4c4d4e"

[statusbar]
fg = "#4f5051"
accent = "#525354"
error = "#555657"
busy = "#58595a"
bg = "transparent"
stats_format = "y {in}"

[picker]
fg = "#5b5c5d"
highlight_bg = "#5e5f60"
highlight_fg = "#616263"
title = "#646566"
dim = "#676869"
bg = "none"

[sidebar]
fg = "#6a6b6c"
heading = "#6d6e6f"
section = "#707172"
badge = "#737475"
warn = "#767778"
error = "#797a7b"
muted = "#7c7d7e"
bg = "transparent"

[dag_band]
fg = "#7f8081"
ok = "#828384"
failed = "#858687"
cancelled = "#88898a"
running = "#8b8c8d"
pending = "#8e8f90"
skipped = "#919293"
edge = "#949596"
title = "#979899"
bg = "transparent"
"##,
    );
    assert_eq!(theme.screen.margin_top, 1);
    assert_eq!(theme.screen.margin_right, 2);
    assert_eq!(theme.screen.margin_bottom, 3);
    assert_eq!(theme.screen.margin_left, 4);
    assert_eq!(theme.feed.gap, 2);
    assert!(theme.feed.separate_all);
    assert_eq!(theme.feed.separator, Some('·'));
    assert_eq!(theme.tool.padding, 3);
    assert_eq!(theme.tool.align, BlockAlign::Left);
    assert_eq!(theme.tool.border_top, BlockBorder::Thick);
    assert_eq!(theme.tool.border_bottom, BlockBorder::Thin);
    assert_eq!(theme.tool.bg, Some(Color::Rgb(0x25, 0x26, 0x27)));
    assert_eq!(theme.thinking.padding, 4);
    assert_eq!(theme.thinking.align, BlockAlign::Right);
    assert!(theme.statusbar.bg.is_none());
    assert!(theme.picker.bg.is_none());
    assert!(theme.sidebar.bg.is_none());
    assert!(theme.dag_band.bg.is_none());
    assert!(theme.thinking_stats_format.is_some());
}
