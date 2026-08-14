# Proposal: tui-grok-render-port

## Why

The TUI (`crates/theway-tui`) renders with ratatui 0.29 + crossterm 0.28 and a
132-line hand-rolled `feed_render.rs`; input editing is upstream
`tui-textarea` 0.7. Users report lag. Grok Build (Apache-2.0,
`/root/workspace/grok-build`) ships the same ratatui/crossterm versions with a
higher-quality primitive layer: a pulldown-cmark-based markdown analyzer, a
forked textarea with edit buffer/undo, low-level render primitives (syntax
highlighting, OSC8 links, scrollbar, image overlays), and an inline-viewport
terminal for embedding the UI in the terminal scroll stream.

## What Changes

Port the reusable primitives into theway as new workspace crates, prefixed
`theway-`, and consume them in `theway-tui`:

| Grok crate | theway crate | Scope |
| --- | --- | --- |
| `xai-grok-markdown-core` | `theway-markdown-core` | full port (single lib.rs, pulldown-cmark only) |
| `xai-ratatui-textarea` | `theway-ratatui-textarea` | full port, replaces `tui-textarea` dep |
| `xai-grok-pager-render` | `theway-pager-render` | selected primitives (syntax, osc8, scrollbar, highlight, theme, line_utils) |
| `xai-ratatui-inline` | `theway-ratatui-inline` | full port — **planning only, gated on user confirmation** |

Phase 0 (pure theway code, no port) wires the existing gRPC event plane into
the TUI and adds incremental feed patching — the lag fix that stands on its
own.

## Non-Goals

- No port of `xai-grok-pager` product code (app/views/ACP/voice/plugins — Grok
  product surface, not reusable primitives).
- No architecture change: the TUI stays a gRPC client of `thewayd`; the
  existing `StreamFrame { snapshot, event }` proto surface already carries
  structured `FeedBlock`s.
- Phase 4 (inline scroll-stream mode) is a UX-form change, not a rendering
  quality fix: it is planned but does not run without explicit user approval.

## Impact

- New crates in the workspace member list; new deps registered in
  `[workspace.dependencies]` (pulldown-cmark, textwrap, unicode-segmentation,
  tui-scrollbar; syntect only if syntax highlighting lands in Phase 3).
- Apache-2.0 components keep their license headers; each ported crate carries
  a `NOTICE` (ratatui-derived code, tui-textarea fork, pulldown-cmark config);
  modified files get an Apache §4(b) change notice.
- `feed_render.rs` shrinks or disappears as Phase 1 lands.
