# Mermaid parser modification rules

This file applies to `crates/mermaid-parser/`. Also follow [`../../AGENTS.md`](../../AGENTS.md) and the ownership boundaries in [`docs/architecture.md`](docs/architecture.md).

## Vendored source

- Keep [`src/parser.rs`](src/parser.rs) monolithic; it is an explicit exception to the workspace 800-line limit.
- Do not mechanically split, format, rename, or clean up [`src/parser.rs`](src/parser.rs), [`src/ir.rs`](src/ir.rs), or [`src/error.rs`](src/error.rs); source comparison depends on small diffs.
- Make an upstream synchronization a separate change and explicitly verify its source and license.
- Attach local lint allowances only at the vendored-code boundary; do not rewrite source code to satisfy workspace style.

## Boundaries

- Keep layout, SVG, font, CLI, and terminal-rendering dependencies out of this parse-only crate.
- Keep theway's `dag_plan` subset policy in [`../theway-core/src/multiagent/graph/mermaid.rs`](../theway-core/src/multiagent/graph/mermaid.rs).
- Preserve stable graph ordering and the public re-exports in [`src/lib.rs`](src/lib.rs) unless the same change updates consumers.
- Record changes to source attribution and licensing in [`LICENSE`](LICENSE) and the crate documentation.

## Validation

Run `cargo test -p mermaid-rs-parser` and `cargo doc -p mermaid-rs-parser --no-deps --document-private-items`. For changes that may affect the DAG adapter, also run `cargo test -p theway-core multiagent::graph::mermaid`.
