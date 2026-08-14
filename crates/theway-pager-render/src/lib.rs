//! Render primitives ported from Grok Build's `xai-grok-pager-render`
//! (Apache-2.0, source revision `5d08d7e`) — the self-contained subset the
//! theway TUI consumes, under `theway-` names.
//!
//! Ported modules:
//! - [`color`] — buffer blend/clear helpers
//! - [`line_utils`] — ratatui line/string utilities
//! - [`scrollbar`] — tui-scrollbar wrapper for feed panes
//! - [`osc8`] — URL/file-path link detection + OSC 8 overlay (terminal
//!   context and link-opening policy localized to theway: http/https only)
//! - [`tool_paths`] — tool-path resolution/shortening helpers
//!
//! Not ported (Grok pager-specific or heavy): syntax (syntect), theme,
//! wrapping/highlight (theme-coupled), glyphs/host/terminal (env detection),
//! preview/image/video overlays, gboom effects. See the crate `NOTICE`.

pub mod color;
pub mod line_utils;
pub mod osc8;
pub mod scrollbar;
pub mod tool_paths;
