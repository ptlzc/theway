//! Thin entry point for the split `tui_render_e2e` test suite. The actual tests live in
//! the `tui_render_e2e/` directory (see `tui_render_e2e/mod.rs`); this wrapper exists
//! because cargo forbids `tests/tui_render_e2e.rs` alongside `tests/tui_render_e2e/`.
//!
//! The vendored `src/tui.rs` references `theway::trigger_engine` by absolute path, so no
//! trigger_engine declaration is needed at the test-crate root.
#[path = "tui_render_e2e/mod.rs"]
mod tui_render_e2e;
