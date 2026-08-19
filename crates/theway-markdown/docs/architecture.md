# Markdown renderer architecture

## Responsibility

`theway-markdown` turns Markdown source into terminal-oriented output. It owns parsing orchestration, presentation transforms, syntax and color adaptation, tables, math, diagrams, link metadata, and incremental-render state; it does not own application feed state or terminal input handling.

## Rendering pipeline

One-shot rendering follows this sequence:

1. [`latex_delimiters.rs`](../src/latex_delimiters.rs) normalizes supported LaTeX delimiters into the canonical forms stored by the renderer.
2. [`parse.rs`](../src/parse.rs) consumes the offset event stream from [`theway-markdown-core`](../../theway-markdown-core/docs/architecture.md) and builds `ParsedMarkdown`.
3. [`render.rs`](../src/render.rs) emits ANSI text or ratatui lines while preserving source-line and source-byte associations.
4. [`url_scan.rs`](../src/url_scan.rs) detects plain URLs that did not originate from Markdown link syntax and appends hyperlink targets.

[`MarkdownRenderOutput`](../src/output.rs) keeps the ratatui lines, source mapping, hyperlinks, code-block spans, and link identifier state together so consumers do not reconstruct metadata from rendered text.

## Streaming model

[`StreamingMarkdownRenderer`](../src/streaming.rs) normalizes input as chunks arrive and stores the normalized source. A checkpoint marks a prefix whose rendered output is stable; later pushes keep that prefix and parse only the mutable tail. Link identifiers and open-code-block highlighting state cross the checkpoint so tail rendering remains consistent with a complete render.

`finish` performs the final tail render and plain-URL scan. For the same normalized source and render settings, finished streaming output and one-shot output must agree in visible content and metadata.

## Specialized transforms

[`syntax.rs`](../src/syntax.rs) selects syntax definitions and themes, while [`colors.rs`](../src/colors.rs) adapts styles to the terminal color level. [`latex/`](../src/latex/mod.rs) converts supported math commands and environments into a Unicode approximation in pretty mode.

[`mermaid.rs`](../src/mermaid.rs) parses a bounded Mermaid subset and lays it out as terminal art. The renderer applies width and complexity limits; unsupported or oversized input becomes a framed source block rather than an unbounded layout operation.

## Boundaries and invariants

- Markdown parser policy belongs to `theway-markdown-core`; this crate must not construct a divergent option set.
- Source ranges use byte offsets into the renderer's normalized source, while line maps identify rendered lines derived from source lines.
- Streaming checkpoints freeze only output that cannot be changed by later source chunks.
- Width calculations use terminal display width and grapheme-aware operations, not byte or scalar counts.
- Diagram and highlighting work remains bounded for model-generated input.
