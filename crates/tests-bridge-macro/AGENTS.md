# Test bridge modification rules

This file applies to `crates/tests-bridge-macro/`. Also follow [`../../AGENTS.md`](../../AGENTS.md) and the test-layout source of truth in [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md).

## Expansion contract

- Keep `#[cfg(test)]` at the call site so the macro can remain a dev-dependency.
- Preserve owning library/binary target detection; an integration target must not compile the mirrored suite again.
- Root generated paths at the calling crate's `CARGO_MANIFEST_DIR/tests` and normalize separators for Rust path literals.
- Reject unsafe or ambiguous input before emitting tokens. Add compile-expansion tests when input parsing, path containment, target detection, or diagnostics change.
- Keep the generated module name `tests` unless the workspace-wide bridge contract changes in the same change.

## Boundaries

- Do not add runtime dependencies, test-runner behavior, fixture loading, or source-module policy to this procedural macro.
- Define standard suite layout and inline-test exceptions only in [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md); this file only references that contract.
- Validate at least one real consumer when expansion semantics change.

## Validation

Run `cargo test -p tests-bridge-macro` and `cargo check --workspace --all-targets`. Run the relevant consumer tests when mirror or target behavior changes.
