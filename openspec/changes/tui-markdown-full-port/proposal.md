# Proposal: tui-markdown-full-port

## Why

Change `tui-grok-render-port` (#24) ported `theway-markdown-core` (pulldown-cmark
analysis) but `theway-tui` only uses it to detect fenced code blocks. Inline
markdown — strong, emphasis, headings, inline code, lists, tables, blockquotes,
strikethrough, links — renders as literal text. The event-to-style renderer is
Grok Build's `xai-grok-markdown` (~20.6k lines: parse / render / streaming /
style / mermaid / latex / syntect highlighting), which #24 scoped out.

## What Changes

Port the full renderer into a new workspace crate and consume it in the TUI feed:

| Grok crate | theway crate | Scope |
| --- | --- | --- |
| `xai-grok-markdown` | `theway-markdown` | full port of `src/` incl. inline tests; drop `bin/*` and the `playground` feature (demo harnesses only) |

Adoption in `crates/theway-tui/src/feed_render.rs`: assistant blocks render
through `theway-markdown` (one-shot `render_markdown_ratatui_full`, pretty
mode), replacing the fenced-code-only path. Feed-level styles (user/thinking/tool
prefixes) stay; the `ai ▸` prefix prepends to the first rendered line. Non-code
lines wrap to the feed width; fenced code lines stay verbatim. Link underlines
come from the renderer's `hyperlinks` output instead of the regex pass.

Syntax highlighting uses syntect with an embedded theme asset
(`grok-night.tmTheme` from `xai-grok-pager-render/assets`, Apache-2.0).

## Decisions

1. **Port = mechanical copy + rename + metadata, verified by tests.** Copy the
   donor `src/`, rewrite `xai_grok_markdown` → `theway_markdown` and the
   `xai-grok-markdown-core` dep → `theway-markdown-core`, set workspace license
   handling (Apache-2.0 + `NOTICE` naming the donor SpaceXAI, source revision
   `5d08d7e`, upstream origins). Behavior changes land as follow-up commits.
2. **File-size governance exception.** Several ported files exceed the ~800-line
   guideline (`mermaid.rs` 5237, `render.rs` 3066, `streaming.rs` 2910,
   `parse.rs` 2127, `latex_delimiters.rs` 1305). They stay in the upstream file
   layout to remain diff-compatible with the donor, mirroring the
   `crates/mermaid-parser` exception.
3. **Playground feature and bins dropped.** `bin/md_*_test.rs` need the optional
   crossterm/textarea deps only for interactive demos; the library surface does
   not use them.
4. **Wrapping semantics.** The renderer emits pre-wrap lines. The TUI wraps
   non-code lines with the existing `wrap_str`; code-block lines (from the
   renderer's `code_blocks` output ranges) and table lines stay verbatim. Grok's
   incremental `frozen_pre_wrap_count` cache is a pager-level optimization and
   not ported — the TUI re-renders only dirty blocks (Phase 0), which keeps the
   per-frame cost bounded.
5. **Streaming path.** Feed snapshots carry complete block text; the TUI renders
   a dirty assistant block one-shot. `StreamingMarkdownRenderer` checkpoint
   streaming is available in the ported crate but is only adopted if profiling
   shows per-frame re-render lag.

## Impact

- New crate in the workspace member list; new deps: syntect 5.3, two-face 0.4
  (syntect-fancy), anstyle 1.0, anstyle-lossy 1.1, anstyle-syntect 1.0,
  html-escape 0.2, supports-color 3.0 (textwrap / unicode-segmentation /
  unicode-width / linkify / url / pulldown-cmark 0.13 already in the lock).
- `feed_render.rs`'s `push_markdown_paragraphs` is replaced; the single-tilde
  literal test flips to parity assertions (strong/emphasis/heading/inline
  code/table/link/fenced-highlight).
- Not a daemon/proto change; the feed wire model is untouched.

## Non-Goals

- No port of `xai-grok-pager` product code (views/ACP/voice — #24 non-goal).
- No inline scroll-stream mode (Phase 4 of #24, still gated on user confirmation).
- No checkpoint-based streaming adoption in the TUI unless profiling demands it.
