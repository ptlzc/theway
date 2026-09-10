# Mermaid parser modification rules

This file contains the complete crate-local modification rules for `mermaid-rs-parser`. Read the ownership boundaries in [`docs/architecture.md`](docs/architecture.md) before changing parser behavior.

## Vendored source

- Keep [`src/parser.rs`](src/parser.rs) monolithic; it is an explicit exception to the workspace 800-line limit.
- Do not mechanically split, format, rename, or clean up [`src/parser.rs`](src/parser.rs), [`src/ir.rs`](src/ir.rs), or [`src/error.rs`](src/error.rs); source comparison depends on small diffs.
- Make an upstream synchronization a separate change and explicitly verify its source and license.
- Attach local lint allowances only at the vendored-code boundary; do not rewrite source code to satisfy workspace style.

## Boundaries

- Keep layout, SVG, font, CLI, and terminal-rendering dependencies out of this parse-only crate.
- Keep theway's `dag_plan` subset policy outside this general-purpose parser crate.
- Preserve stable graph ordering and the public re-exports in [`src/lib.rs`](src/lib.rs) unless the same change updates consumers.
- Record changes to source attribution and licensing in [`LICENSE`](LICENSE) and the crate documentation.

## Validation

This crate is excluded from the root workspace and declares its own `[workspace]`; run these checks from the repository root with `--manifest-path`.

Run `cargo test --manifest-path crates/mermaid-parser/Cargo.toml` and `cargo doc --manifest-path crates/mermaid-parser/Cargo.toml --no-deps --document-private-items`. For changes that may affect the DAG adapter, also run `cargo test --manifest-path crates/theway-core/Cargo.toml multiagent::graph::mermaid`.
