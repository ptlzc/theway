# theway-markdown

English | [中文](README.zh.md)

`theway-markdown` renders complete or incrementally arriving Markdown for terminal applications. It produces ANSI text or styled ratatui lines and carries source maps, hyperlink targets, code-block spans, and stable streaming checkpoints alongside the visual output.

## Capabilities

- Incremental rendering with a frozen prefix and a re-rendered mutable tail.
- GFM tables, task lists, code blocks, inline styles, block quotes, and lists.
- Syntax highlighting with terminal color-level adaptation.
- Unicode approximations for supported LaTeX math syntax.
- Width-bounded Mermaid rendering with a source fallback for unsupported or oversized diagrams.
- OSC 8 hyperlink metadata and source-to-rendered-line mapping.

The parser feature set and double-tilde-only strikethrough policy come from `theway-markdown-core`. Applications should use [`StreamingMarkdownRenderer`](src/streaming.rs) for token streams and the one-shot functions in [`src/lib.rs`](src/lib.rs) for complete documents.

## Development

The rendering pipeline and streaming contract are documented in [`docs/architecture.md`](docs/architecture.md). Directory-specific modification rules are in [`AGENTS.md`](AGENTS.md), and code lineage is recorded in [`NOTICE`](NOTICE).

Run the crate checks from the workspace root:

```bash
cargo test -p theway-markdown
cargo doc -p theway-markdown --no-deps --document-private-items
```
