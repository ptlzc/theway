# Markdown core architecture

## Responsibility

`theway-markdown-core` owns the shared, terminal-independent interpretation of Markdown source. It has one runtime dependency, `pulldown-cmark`, so callers can perform policy-consistent analysis without importing the presentation stack.

## Parser policy

[`parser_options`](../src/lib.rs) is the source of truth for enabled Markdown extensions. [`offset_events`](../src/lib.rs) creates the parser from those options and retains each event's byte range in the original input.

`pulldown-cmark` recognizes single-tilde pairs as strikethrough when the extension is enabled. `DoubleTildeOnlyStrike` converts the opening and closing tags for those single-tilde pairs into literal delimiter text while leaving `~~double tilde~~` tags intact. The transformation is stackless because matching start and end events carry the same source span.

## Analysis

[`analyze`](../src/lib.rs) consumes the same offset event stream as the renderer and returns two kinds of information:

- [`MarkdownStats`](../src/lib.rs) counts parsed constructs such as headings, code blocks, tables, links, images, math, and list items.
- [`StructuralIssue`](../src/lib.rs) identifies source that indicates an intended construct but parses with degraded structure, currently malformed GFM tables and unterminated fenced code blocks.

CommonMark parsing is total, so structural diagnostics compare raw source intent with parsed events rather than reporting parser errors. Diagnostic checks must remain bounded and must not change the event stream consumed by renderers.

## Boundaries and invariants

- Source offsets are UTF-8 byte ranges into the input passed to `offset_events` or `analyze`.
- Parser extensions and the single-tilde rule have one implementation in this crate.
- The crate contains no terminal, color, widget, syntax-theme, or application state.
- Statistics describe parsed structure; structural issues describe likely render-fidelity failures.
