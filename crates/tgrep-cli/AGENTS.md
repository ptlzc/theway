# tgrep-cli modification rules

This file contains the complete crate-local modification rules for `tgrep-cli`. Read the ownership boundaries in [`docs/architecture.md`](docs/architecture.md) before changing anything here.

## Vendored source

- Sources under `src/` (and `tests/`, `build.rs`) are **verbatim** copies of
  upstream [microsoft/tgrep](https://github.com/microsoft/tgrep) at commit
  `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`. Do not edit them.
- Do not format, rename, refactor, or "fix lint warnings" in vendored sources;
  source comparison against upstream depends on zero local diffs.
- `Cargo.toml` is self-contained (not verbatim): theway's workspace has no
  `[workspace.dependencies]`, and vendored crates keep independent package
  metadata. On upgrades, re-apply only the version/dependency values upstream
  changed; never add theway-specific dependencies here.
- Make an upstream synchronization a separate change, pin the new commit in
  `README.md`/`README.zh.md`, and verify `diff -r` against the upstream
  tarball (sources byte-identical) before pushing.
- Report bugs upstream (https://github.com/microsoft/tgrep/issues) instead of
  patching vendored code. If a patch is unavoidable in an emergency, mark it
  clearly and file the upstream issue in the same change.

## Boundaries

- The `tgrep` binary is consumed by the daemon's `TgrepServerRegistry`
  (`crates/theway-daemon/src/tgrep_server.rs`, issue #121): the daemon spawns
  `tgrep serve <root>` and runs `tgrep ... --json` client queries for the
  built-in grep tool. Do not change the CLI surface (flags / JSON output
  format) here — those contracts are upstream's.
- Keep theway-specific orchestration (spawn/readiness/LRU/reaping) out of this
  crate; it belongs in the daemon.

## Validation

Run `cargo test -p tgrep-cli` and `cargo check -p tgrep-cli`. Integration
tests in this crate exercise the CLI end to end against tempdir fixtures.
