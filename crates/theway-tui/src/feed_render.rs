//! Ratatui rendering for the conversation feed — the terminal rendering lives
//! here in the TUI; the UI-agnostic model lives in `theway_transport::feed`.
//!
//! [`lines`] renders a [`Feed`] to width-wrapped, styled `ratatui` lines,
//! ready to scroll/draw — the terminal counterpart of `Feed::plain_lines`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use theway_transport::feed::{Block, Level, block_fingerprint, display_prefix, wrap_str};

use crate::ui::theme::{BlockAlign, BlockBorder, BlockTheme, Theme};

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/styles.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/markdown.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/block.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/window.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/incremental_wrap.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/links.rs"
));

#[cfg(test)]
tests_bridge_macro::tests_bridge!("feed_render/unit");

#[cfg(test)]
mod feed_render_property_tests {
    tests_bridge_macro::tests_bridge!("feed_render/properties");
}
