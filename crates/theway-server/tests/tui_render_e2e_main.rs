//! Thin entry point for the split `tui_render_e2e` test suite. The actual tests live in
//! the `tui_render_e2e/` directory (see `tui_render_e2e/mod.rs`); this wrapper exists
//! because cargo forbids `tests/tui_render_e2e.rs` alongside `tests/tui_render_e2e/`.
//!
//! `trigger_engine` must be declared here at the test-crate root (not inside the
//! `tui_render_e2e` module): the vendored `src/tui.rs` and `src/trigger_engine/*` files
//! reference `crate::trigger_engine` by absolute path.
#[allow(dead_code)]
#[path = "../src/trigger_engine/mod.rs"]
mod trigger_engine;

#[path = "tui_render_e2e/mod.rs"]
mod tui_render_e2e;
