//! Integration test for the slash-command registry. Drives `dispatch` against a real
//! `AgentHarness` (faux stream) and verifies user-visible effects: `/thinking high` flips the
//! harness's thinking level *and* writes a thinking_level_change row to the session, so
//! `--resume` later restores it.
//!
//! Split into domain modules under `commands_e2e/` (file-size governance: >800 lines →
//! directory split, see docs/RUST_TEST_FILES.md). The `src/` path-include block must stay
//! here at the test-crate root: the included modules reference each other through
//! `crate::...`, and the `commands` macros expand to `$crate::commands::...`.

// The binary crate doesn't expose `commands` — pull it in via path-include so this test
// exercises the actual code path without restructuring the crate as a [lib]. `commands.rs`
// references sibling modules through `crate::...`, so we include those siblings too. They appear unused-from-tests
// (no items are called directly here) — that's fine; the commands module reaches into them.
#[allow(dead_code)]
#[path = "../src/auth.rs"]
mod auth;
#[allow(dead_code)]
#[path = "../src/bug_report.rs"]
mod bug_report;
#[path = "../src/commands/mod.rs"]
mod commands;
#[allow(dead_code)]
#[path = "../src/trigger_engine/mod.rs"]
mod trigger_engine;

#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/export.rs"]
mod export;
#[allow(dead_code)]
#[path = "../src/history.rs"]
mod history;
#[allow(dead_code)]
#[path = "../src/inbox.rs"]
mod inbox;
#[allow(dead_code)]
#[path = "../src/mcp_loader.rs"]
mod mcp_loader;
#[allow(dead_code)]
#[path = "../src/session/mod.rs"]
mod session;
#[allow(dead_code)]
#[path = "../src/session_archive.rs"]
mod session_archive;
#[allow(dead_code)]
#[path = "../../core/src/skills_state.rs"]
mod skills_state;
#[allow(dead_code)]
#[path = "../src/triggers/mod.rs"]
mod triggers;

#[path = "commands_e2e/mod.rs"]
mod commands_e2e;
