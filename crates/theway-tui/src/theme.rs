//! TUI theme: color roles + block layout + composer style + feed rhythm
//! (issues #43 + #49 + #30).
//!
//! [`Theme::load`] parses `${THEWAY_DIR:-$HOME/.theway}/theme.toml` once at
//! startup. Sections: `[colors]` roles, `[blocks.<kind>]` layout,
//! `[composer]` chrome, `[feed]` vertical rhythm, `[palette]` named colors.
//! [`Theme::default`] mirrors the hardcoded tokyonight consts in
//! [`crate::feed_render`] / [`crate::ui::prompt_chrome`] exactly, so a build
//! without a theme file renders identically to the pre-theme build.
//!
//! v2 (issue #30) upgrades parsing to the `toml` crate while keeping the v1
//! key set and semantics byte-identical. New in v2:
//! - `[palette]` — named colors referenced from any color slot as `p:name`.
//! - Extended color literals: short hex `#7AF`, ANSI names (`red`,
//!   `lightBlue`, `default`), 256-palette indexes (`"146"`), and
//!   `transparent` / `none` to clear optional (background) slots.
//! - `[feed] gap` / `separator` / `separator_style` — the inter-block
//!   vertical rhythm that v1 could not express.
//!
//! Forgiving posture (kept from v1): unknown sections/keys, invalid hex and
//! invalid values warn on stderr and keep the current value (the default
//! unless overridden earlier); missing sections/keys and a missing file all
//! mean "default".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::Color;
use toml::Table as TomlTable;

use crate::feed_render::{
    ASSISTANT_PREFIX_DEFAULT, ASSISTANT_TEXT_DEFAULT, THINKING_BG_DEFAULT, THINKING_TEXT_DEFAULT,
    TOOL_ARGS_DEFAULT, TOOL_ERROR_BG_DEFAULT, TOOL_ERROR_DEFAULT, TOOL_RESULT_DEFAULT,
    TOOL_RUNNING_BG_DEFAULT, TOOL_SUCCESS_BG_DEFAULT, TOOL_TITLE_DEFAULT, USER_BG_DEFAULT,
    USER_TEXT_DEFAULT,
};
use crate::ui::prompt_chrome::{
    ACCENT_USER, BG_BASE, BORDER_FOCUSED, BORDER_UNFOCUSED, GRAY, GRAY_DIM, TEXT_PRIMARY,
    TEXT_SECONDARY,
};

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/theme/components.rs"
));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/theme/core.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/theme/color.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/theme/sections.rs"
));

#[cfg(test)]
// Test files live in `tests/theme/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("theme");
