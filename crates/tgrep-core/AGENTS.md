# tgrep-core modification rules

This file contains the complete crate-local modification rules for `tgrep-core`. Read the ownership boundaries in [`docs/architecture.md`](docs/architecture.md) before changing anything here.

## Vendored source

- Sources under `src/` (and `benches/`) are **verbatim** copies of upstream
  [microsoft/tgrep](https://github.com/microsoft/tgrep) at commit
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

- Keep the client/server logic out of this crate: `tgrep-core` is the index
  library (builder/query/reader/walker); the serve/client orchestration lives
  in `crates/tgrep-cli`.
- Theway's daemon does not depend on `tgrep-core` directly — it shells out to
  the `tgrep` binary. Do not add a `tgrep-core` dependency to runtime crates
  without a dedicated change.

## Validation

This crate is excluded from the root workspace and declares its own `[workspace]`; run these checks from the repository root with `--manifest-path`.

Run `cargo test --manifest-path crates/tgrep-core/Cargo.toml` and `cargo check --manifest-path crates/tgrep-core/Cargo.toml`. The vendored bench suite runs with `cargo bench --manifest-path crates/tgrep-core/Cargo.toml`.
