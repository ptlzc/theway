# tests-bridge-macro

English | [中文](README.zh.md)

`tests-bridge-macro` provides the `tests_bridge!` procedural macro, which attaches a mirrored multi-file test suite to its owning source module. It solves the requirement that `#[path]` accept only a literal path while preserving unit-test access to private items.

A call in a source module:

```rust,ignore
#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/session");
```

expands in the owning crate's unit-test target to an absolute `#[path = "…/tests/agent/session/mod.rs"] mod tests;` rooted at `CARGO_MANIFEST_DIR`. The source call site owns `#[cfg(test)]`, and the macro is normally used as a dev-dependency.

When an integration-test crate imports the same source by path, the macro compares the Cargo target environment and emits no tokens. This prevents the mirrored suite from compiling under two crate roots or racing over process-global test state.

Test layout and bridge placement are defined centrally in [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md). The expansion mechanism is documented in [`docs/architecture.md`](docs/architecture.md), and modification rules are in [`AGENTS.md`](AGENTS.md).

## Validation

```bash
cargo test -p tests-bridge-macro
cargo check --workspace --all-targets
```
