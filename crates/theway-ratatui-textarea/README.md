# theway-ratatui-textarea

English | [中文](README.zh.md)

`theway-ratatui-textarea` is a reusable multiline editor and ratatui widget. It provides grapheme-aware editing, soft wrapping, selection, mouse interaction, clipboard integration, undo and redo, scrolling, and atomic text elements such as indivisible application-inserted spans.

## Public API

- [`EditBuffer`](src/editor.rs) and [`EditPlan`](src/editor.rs) provide UI-independent edit planning and validated application.
- [`TextArea`](src/textarea.rs) configures the widget, while [`TextAreaState`](src/textarea.rs) owns mutable text, cursor, selection, history, scrolling, and element state.
- [`TextElement`](src/textarea.rs) marks an atomic range and [`TextElementEvent`](src/textarea.rs) reports element interactions to the application.
- [`ClipboardProvider`](src/textarea.rs) lets the embedding application choose system or internal clipboard behavior.

The example in [`examples/textarea_demo.rs`](examples/textarea_demo.rs) demonstrates keyboard input, selection, search, rendering, and clipboard wiring.

## Development

The editor, widget, wrapping, and rendering layers are documented in [`docs/architecture.md`](docs/architecture.md). Directory-specific modification rules are in [`AGENTS.md`](AGENTS.md), and code lineage is recorded in [`NOTICE`](NOTICE).

This crate is excluded from the root workspace and declares its own `[workspace]`; run these checks from the repository root with `--manifest-path`.

```bash
cargo test --manifest-path crates/theway-ratatui-textarea/Cargo.toml
cargo check --manifest-path crates/theway-ratatui-textarea/Cargo.toml --example textarea_demo
cargo doc --manifest-path crates/theway-ratatui-textarea/Cargo.toml --no-deps --document-private-items
```
