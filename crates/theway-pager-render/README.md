# theway-pager-render

`theway-pager-render` provides reusable ratatui rendering primitives for feed and pager-style views. It deliberately contains no session state, daemon protocol, or application event loop.

## Modules

- [`color`](src/color.rs) blends and clears terminal buffer regions.
- [`line_utils`](src/line_utils.rs) measures, slices, truncates, and converts styled ratatui lines using terminal display width.
- [`scrollbar`](src/scrollbar.rs) adapts `tui-scrollbar` for feed panes.
- [`osc8`](src/osc8.rs) detects safe web links and file references and overlays OSC 8 metadata.
- [`tool_paths`](src/tool_paths.rs) resolves and shortens tool-reported paths for display.

The TUI composes these primitives with its own view state and interaction policy. Link activation remains an application responsibility; this crate only recognizes and annotates targets.

## Development

The module boundaries and safety rules are documented in [`docs/architecture.md`](docs/architecture.md). Directory-specific modification rules are in [`AGENTS.md`](AGENTS.md), and code lineage is recorded in [`NOTICE`](NOTICE).

Run the crate checks from the workspace root:

```bash
cargo test -p theway-pager-render
cargo doc -p theway-pager-render --no-deps --document-private-items
```
