# theway-markdown-core

English | [中文](README.zh.md)

`theway-markdown-core` is the headless Markdown policy and analysis crate. It exposes the parser configuration used by the terminal renderer, an offset-preserving event stream, element statistics, and structural diagnostics without depending on ratatui, syntax highlighting, or terminal capabilities.

## Public API

- [`parser_options`](src/lib.rs) defines the enabled `pulldown-cmark` extensions: GFM, tables, task lists, math, and strikethrough.
- [`offset_events`](src/lib.rs) returns parser events with source byte ranges and preserves the project rule that only `~~double tilde~~` is strikethrough.
- [`analyze`](src/lib.rs) produces [`MarkdownAnalysis`](src/lib.rs), combining [`MarkdownStats`](src/lib.rs) with render-fidelity [`StructuralIssue`](src/lib.rs) values.

Use this crate when a caller needs to inspect Markdown without pulling in `theway-markdown`. Consumers that render Markdown should also use `offset_events` instead of constructing an independent parser so analysis and rendering interpret the same source consistently.

## Development

The mechanism and invariants are documented in [`docs/architecture.md`](docs/architecture.md). Directory-specific modification rules are in [`AGENTS.md`](AGENTS.md), and code lineage is recorded in [`NOTICE`](NOTICE).

Run the crate checks from the workspace root:

```bash
cargo test -p theway-markdown-core
cargo doc -p theway-markdown-core --no-deps --document-private-items
```
