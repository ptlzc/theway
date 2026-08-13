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
// Moved to the SDK (sdk-split-local-sandbox): forwarded instead of path-included.
#[allow(dead_code)]
mod auth {
    #[allow(unused_imports)]
    pub use theway::auth::*;
    // Test-only env serialization lock. The SDK keeps a `pub(crate)` one for its own
    // suites; each test binary serializes its own env mutations with this copy.
    pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
// Shared env lock + guard for the bridged `commands` unit tests (which mutate
// `THEWAY_DIR` / provider keys); mirrors the `crate::test_env` bridge in
// `src/lib.rs` for the lib test target. See tests/env_lock.rs.
#[allow(dead_code)]
#[path = "../src/bug_report.rs"]
mod bug_report;
#[path = "../src/commands/mod.rs"]
mod commands;
#[path = "common/env_lock.rs"]
pub(crate) mod test_env;
#[allow(dead_code)]
#[path = "../src/trigger_engine/mod.rs"]
mod trigger_engine;

#[allow(dead_code)]
mod config {
    #[allow(unused_imports)]
    pub use theway_transport::config::*;
}
#[allow(dead_code)]
#[path = "../src/export.rs"]
mod export;
#[allow(dead_code)]
mod history {
    #[allow(unused_imports)]
    pub use theway::history::*;
}
#[allow(dead_code)]
#[path = "../src/mcp_loader.rs"]
mod mcp_loader;
#[allow(dead_code)]
mod session {
    #[allow(unused_imports)]
    pub use theway::session::*;
}
#[allow(dead_code)]
mod session_archive {
    #[allow(unused_imports)]
    pub use theway::session_archive::*;
}
#[allow(dead_code)]
#[path = "../../theway-core/src/skill_overrides.rs"]
mod skill_overrides;
#[allow(dead_code)]
#[path = "../src/triggers/mod.rs"]
mod triggers;

#[path = "commands_e2e/mod.rs"]
mod commands_e2e;
