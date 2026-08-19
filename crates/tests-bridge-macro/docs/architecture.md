# Test bridge architecture

English | [中文](architecture.zh.md)

## Responsibility

During macro expansion, `tests-bridge-macro` turns a mirrored test path into a module declaration anchored at the crate root. It provides path anchoring only; the owning source module decides whether tests compile, and the suite lives under `tests/<mirrored-src-path>/`.

## Expansion flow

[`tests_bridge`](../src/lib.rs) performs these steps:

1. Read `CARGO_CRATE_NAME`, `CARGO_PKG_NAME`, and `CARGO_BIN_NAME`, then normalize hyphens in package and binary names to underscores.
2. Return an empty token stream when the active target is not the package's own library or binary unit-test target.
3. Convert the input token stream to a string, strip surrounding quote characters, and reject an empty mirror or a mirror containing `..`.
4. Join `CARGO_MANIFEST_DIR`, `tests`, the mirror, and `mod.rs`, normalize separators to forward slashes, and emit `#[path = "<absolute path>"] mod tests;`.

The call contract is a quoted relative mirror such as `"agent/session"`. The steps above enumerate the validation the macro actually performs; changes to input parsing or path containment require dedicated compile-expansion coverage.

## Target filtering

An integration test can import a source module by path while `cfg(test)` is enabled. In that case, `CARGO_CRATE_NAME` names the integration target rather than the owning package or binary, so the macro expands to nothing. The original suite compiles once under the library or binary crate root and retains unit-test visibility.

## Boundaries and invariants

- The source call site retains `#[cfg(test)]`; production compilation does not require the dev-dependency.
- The generated module name remains `tests`, and the target file remains `mod.rs` under the mirror.
- Expansion depends only on the compile-time Cargo environment and performs no runtime work.
- The macro has no non-std dependencies and contains no test-discovery or execution policy.
