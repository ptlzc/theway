//! Markdown renderer - transforms parsed markdown buffers into styled output.
//!
//! After parsing with `MarkdownParser`, use `ParsedMarkdown` to render
//! to either ratatui Lines or ANSI strings.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt::Write as FmtWrite;
use std::ops::Range;

use anstyle::{Effects, Reset, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::Style as SyntectStyle;

use crate::buffers::{MarkdownBuffers, RenderEvent, RenderEventKind, unicode_display_width};
use crate::checkpoint::Checkpoint;
use crate::colors::{ColorLevel, adapt_style_for};
use crate::hyperlinks::{ChunkLinkRange, chunk_link_offsets, emit_segment_hyperlinks};
use crate::output::{HyperlinkTarget, MarkdownRenderOutput};
use crate::parse::ParsedMarkdown;
use crate::source_map::SourceMap;
use crate::style::{all_hidden, merge_styles};

/// Trait for converting anstyle to ratatui style.
trait StyleInto<T> {
    fn style_into(self) -> T;
}

impl StyleInto<ratatui::style::Style> for Style {
    fn style_into(self) -> ratatui::style::Style {
        use ratatui::style::{Modifier, Style as RStyle};

        let mut style = RStyle::default();

        if let Some(fg) = self.get_fg_color() {
            style = style.fg(anstyle_to_ratatui_color(fg));
        }
        if let Some(bg) = self.get_bg_color() {
            style = style.bg(anstyle_to_ratatui_color(bg));
        }

        let effects = self.get_effects();
        let mut modifiers = Modifier::empty();
        if effects.contains(Effects::BOLD) {
            modifiers |= Modifier::BOLD;
        }
        if effects.contains(Effects::DIMMED) {
            modifiers |= Modifier::DIM;
        }
        if effects.contains(Effects::ITALIC) {
            modifiers |= Modifier::ITALIC;
        }
        if effects.contains(Effects::UNDERLINE) {
            modifiers |= Modifier::UNDERLINED;
        }
        if effects.contains(Effects::STRIKETHROUGH) {
            modifiers |= Modifier::CROSSED_OUT;
        }
        if effects.contains(Effects::HIDDEN) {
            modifiers |= Modifier::HIDDEN;
        }

        style.add_modifier(modifiers)
    }
}

fn anstyle_to_ratatui_color(color: anstyle::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        anstyle::Color::Ansi(ansi) => match ansi {
            anstyle::AnsiColor::Black => Color::Black,
            anstyle::AnsiColor::Red => Color::Red,
            anstyle::AnsiColor::Green => Color::Green,
            anstyle::AnsiColor::Yellow => Color::Yellow,
            anstyle::AnsiColor::Blue => Color::Blue,
            anstyle::AnsiColor::Magenta => Color::Magenta,
            anstyle::AnsiColor::Cyan => Color::Cyan,
            anstyle::AnsiColor::White => Color::Gray,
            anstyle::AnsiColor::BrightBlack => Color::DarkGray,
            anstyle::AnsiColor::BrightRed => Color::LightRed,
            anstyle::AnsiColor::BrightGreen => Color::LightGreen,
            anstyle::AnsiColor::BrightYellow => Color::LightYellow,
            anstyle::AnsiColor::BrightBlue => Color::LightBlue,
            anstyle::AnsiColor::BrightMagenta => Color::LightMagenta,
            anstyle::AnsiColor::BrightCyan => Color::LightCyan,
            anstyle::AnsiColor::BrightWhite => Color::White,
        },
        anstyle::Color::Ansi256(idx) => Color::Indexed(idx.index()),
        anstyle::Color::Rgb(rgb) => Color::Rgb(rgb.0, rgb.1, rgb.2),
    }
}

/// Render raw highlighted spans to an ANSI string.
fn render_replace_ansi(
    highlighted: &[Vec<(SyntectStyle, String)>],
    color_level: ColorLevel,
) -> String {
    let mut out = String::new();
    for line_spans in highlighted {
        for (style, text) in line_spans {
            if text.is_empty() {
                continue;
            }
            let full_style = anstyle_syntect::to_anstyle(*style);
            let fg_only = full_style.bg_color(None);
            let adapted = adapt_style_for(fg_only, color_level);
            if adapted != Style::new() {
                write!(out, "{adapted}{text}\x1b[0m").ok();
            } else {
                out.push_str(text);
            }
        }
    }
    out
}

/// Stylize trait for ANSI rendering.
trait Stylize {
    fn astyle(&self, style: Style) -> StyledStr<'_>;
}

impl Stylize for str {
    fn astyle(&self, style: Style) -> StyledStr<'_> {
        StyledStr { text: self, style }
    }
}

struct StyledStr<'a> {
    text: &'a str,
    style: Style,
}

impl<'a> std::fmt::Display for StyledStr<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.style.is_plain() {
            write!(f, "{}", self.text)
        } else {
            write!(f, "{}{}\x1b[0m", self.style, self.text)
        }
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/render/transforms.rs"
));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/render/events.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/render/ansi.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/render/ratatui.rs"
));

#[cfg(test)]
tests_bridge_macro::tests_bridge!("render/unit");
#[cfg(test)]
mod render_extra_tests {
    tests_bridge_macro::tests_bridge!("render/extra");
}
