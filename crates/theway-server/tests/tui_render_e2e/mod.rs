//! Capture-test for the TUI renderer. Drives a realistic event sequence (thinking → text →
//! tool call → tool result → final text → agent end) and asserts on the captured byte
//! stream. Stand-in for a real terminal e2e: without a TTY we can't observe cursor moves,
//! but we can pin the textual content + ANSI escapes that get emitted in order, which
//! catches the bugs we hit live (spinner remnants, double-printed tool names, stale color
//! formatting bleeding into post-thinking text).
//!
//! Split into domain submodules: [`helpers`] (fixtures), [`rendering`] (text rendering
//! tests) and [`trigger`] (trigger / dynamic-poll rendering tests). The test harness entry
//! is `tests/tui_render_e2e_main.rs`, which includes this module via `#[path]`.
//!
//! Note: `trigger_engine` is declared in the entry file (`tui_render_e2e_main.rs`) at the
//! test-crate root because the vendored `src/tui.rs` references `crate::trigger_engine`.

#[allow(dead_code)]
#[path = "../../src/tui.rs"]
mod tui;

pub mod helpers;
pub mod rendering;
pub mod trigger;
