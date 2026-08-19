# mermaid-rs-parser

English | [中文](README.zh.md)

`mermaid-rs-parser` is the vendored Mermaid parsing stage used by theway. It transforms Mermaid source text into a structured [`Graph`](src/ir.rs) without an SVG renderer, layout engine, font stack, or CLI.

The public [`parse_mermaid`](src/parser.rs) function returns [`ParseOutput`](src/parser.rs), which contains the parsed graph and an optional initialization directive. The intermediate representation records the detected diagram kind, direction, nodes, edges, subgraphs, and diagram-specific data.

## Consumer contract

The `theway-core` DAG adapter accepts a smaller flowchart contract for `dag_plan`. The adapter preprocesses DAG node identifiers and line-level syntax, invokes this crate for Mermaid parsing, then restores identifiers and derives the agent, task, and dependency fields. Subset validation belongs to the adapter; this crate remains a general parsing stage.

## Vendored source

[`src/parser.rs`](src/parser.rs), [`src/ir.rs`](src/ir.rs), and [`src/error.rs`](src/error.rs) contain parsing-stage code sourced from `mermaid-rs-renderer` (`mmdr`) 0.3.1. [`src/lib.rs`](src/lib.rs) and [`Cargo.toml`](Cargo.toml) form the local crate shell. Upstream attribution and licensing are recorded in [`LICENSE`](LICENSE).

The vendored parser stays in one file so it can be compared with its source. Read [`AGENTS.md`](AGENTS.md) before modifying it; the parser and DAG adapter boundary is documented in [`docs/architecture.md`](docs/architecture.md).

## Validation

```bash
cargo test -p mermaid-rs-parser
cargo doc -p mermaid-rs-parser --no-deps --document-private-items
cargo run -p mermaid-rs-parser --example dag_plan_demo
```
