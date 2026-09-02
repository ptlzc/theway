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
            "separate_all" => match value.as_bool() {
                Some(flag) => feed.separate_all = flag,
                None => warn(&format!(
                    "feed.separate_all: invalid value {value:?} — keeping the current value"
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
    string_slots: &mut [(&str, &mut Option<&'static str>)],
) {
    for (key, value) in section {
        let Some(value) = as_str(&format!("{label}.{key}"), value) else {
            continue;
        };
        if let Some(slot) = opt_slots.iter_mut().find(|(name, _)| *name == key) {
            set_opt_color(slot.1, &format!("{label}.{key}"), value, palette);
            continue;
        }
        if let Some(slot) = string_slots.iter_mut().find(|(name, _)| *name == key) {
            *slot.1 = Some(Box::leak(value.to_string().into_boxed_str()));
            continue;
        }
        if let Some(slot) = color_slots.iter_mut().find(|(name, _)| *name == key) {
            set_color(slot.1, &format!("{label}.{key}"), value, palette);
            continue;
        }
        warn(&format!("{label}.{key}: unknown key — ignored"));
    }
}
