# theway-ratatui-textarea

`theway-ratatui-textarea` is a reusable multiline editor and ratatui widget. It provides grapheme-aware editing, soft wrapping, selection, mouse interaction, clipboard integration, undo and redo, scrolling, and atomic text elements such as indivisible application-inserted spans.

## Public API

- [`EditBuffer`](src/editor.rs) and [`EditPlan`](src/editor.rs) provide UI-independent edit planning and validated application.
- [`TextArea`](src/textarea.rs) configures the widget, while [`TextAreaState`](src/textarea.rs) owns mutable text, cursor, selection, history, scrolling, and element state.
- [`TextElement`](src/textarea.rs) marks an atomic range and [`TextElementEvent`](src/textarea.rs) reports element interactions to the application.
- [`ClipboardProvider`](src/textarea.rs) lets the embedding application choose system or internal clipboard behavior.

The example in [`examples/textarea_demo.rs`](examples/textarea_demo.rs) demonstrates keyboard input, selection, search, rendering, and clipboard wiring.

## Development

The editor, widget, wrapping, and rendering layers are documented in [`docs/architecture.md`](docs/architecture.md). Directory-specific modification rules are in [`AGENTS.md`](AGENTS.md), and code lineage is recorded in [`NOTICE`](NOTICE).

Run the crate checks from the workspace root:

```bash
cargo test -p theway-ratatui-textarea
cargo check -p theway-ratatui-textarea --example textarea_demo
cargo doc -p theway-ratatui-textarea --no-deps --document-private-items
```
