//! `tests_bridge` — compile-time "top-level anchor" for module tests.
//!
//! Rust's `#[path]` attribute only accepts string literals (relative to the
//! containing source file), and the attribute evaluator rejects macro calls
//! (`#[path = concat!(...)]` → `malformed path attribute input`; RFC 2320
//! "eager macro expansion" was closed unmerged). This macro sidesteps the
//! limitation by expanding — at macro-expansion time, before attribute
//! evaluation — into a `#[path = "<absolute>"] mod tests;` where the path is
//! anchored at `CARGO_MANIFEST_DIR` (the *calling* crate's root, which is the
//! environment the proc-macro runs in). That is the TS `@/`-equivalent Rust
//! does not provide natively.
//!
//! Usage (in the crate whose `tests/<mirror>/mod.rs` holds the module tests):
//!
//! ```ignore
//! #[cfg(test)]
//! tests_bridge_macro::tests_bridge!("runtime/multiagent/graph/engine");
//! // expands to:
//! //   #[cfg(test)] #[path = "/abs/<CARGO_MANIFEST_DIR>/tests/runtime/multiagent/graph/engine/mod.rs"] mod tests;
//! ```
//!
//! The call site keeps the `#[cfg(test)]` prefix so plain `cargo build` skips
//! the expansion entirely (the macro is a dev-dependency and unavailable to
//! non-test builds). Mirror path is relative to the crate root, `..` rejected.

use proc_macro::TokenStream;
use std::env;
use std::path::PathBuf;

#[proc_macro]
pub fn tests_bridge(input: TokenStream) -> TokenStream {
    // Only bridge tests when compiling the package's own library or binary
    // unit-test target.
    // Integration-test crates often path-include source modules (to reach private
    // code / macros); in that context `cfg(test)` is also true, but the bridge's
    // `crate::...` paths refer to the test-crate root instead of the lib, and the
    // bridged tests run a second time in a binary that already has its own
    // integration tests. That duplicate execution also races on process-global
    // state (console sinks, registries, env locks). Compare CARGO_CRATE_NAME to
    // CARGO_PKG_NAME or CARGO_BIN_NAME so the bridge is emitted for the owning
    // target, not for `tests/*.rs` integration binaries.
    let crate_name = env::var("CARGO_CRATE_NAME").unwrap_or_default();
    let package_name = env::var("CARGO_PKG_NAME")
        .unwrap_or_default()
        .replace('-', "_");
    let bin_name = env::var("CARGO_BIN_NAME")
        .unwrap_or_default()
        .replace('-', "_");
    if crate_name != package_name && crate_name != bin_name {
        return TokenStream::new();
    }

    let lit = input.to_string().trim().to_string();
    let mirror = lit.trim_matches('"').to_string();
    assert!(
        !mirror.is_empty() && !mirror.contains(".."),
        "tests_bridge!: mirror path must be a non-empty literal without `..`, got `{lit}`"
    );
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    // `tests/<mirror>/mod.rs`, anchored at the calling crate root. Forward slashes
    // keep the generated literal portable across platforms.
    let abs = manifest
        .join("tests")
        .join(&mirror)
        .join("mod.rs")
        .to_string_lossy()
        .replace('\\', "/");
    format!(r#"#[path = "{abs}"] mod tests;"#)
        .parse()
        .expect("generated bridge tokens must parse")
}
