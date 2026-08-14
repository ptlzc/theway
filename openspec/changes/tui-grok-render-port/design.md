# Design: tui-grok-render-port

## Context

`theway-tui` is a pure gRPC client (see change `tui-connect-daemon`): it
renders from `WireStatus` snapshots and drops `StreamEvent` frames
(`ui/mod.rs` `apply_frame`). Rendering is ratatui 0.29 + crossterm 0.28, feed
blocks drawn by the 132-line `feed_render.rs`; input is upstream
`tui-textarea` 0.7. The daemon owns the transcript; `FeedBlock`
(user/assistant/thinking/tool/tool_result/plain) is already structured in
`proto/theway_grpc.proto`.

Grok Build lives at `/root/workspace/grok-build` (Apache-2.0, revision
`5d08d7e`). Its UI primitive crates run on the same ratatui 0.29 / crossterm
0.28 versions as theway, so a port is a dependency-level exercise, not a
framework migration.

## Goals / Non-Goals

**Goals:**

- Phase 0 removes the lag sources in theway code: apply `StreamEvent` frames
  as increments, diff `FeedBlock`s instead of wholesale replace, cache feed
  layout across frames.
- Phases 1–3 port the Grok primitive crates under `theway-` names and consume
  them: markdown analysis + rendering (1), input editing (2), selected render
  primitives (3).
- Phase 4 is planned (inline scroll-stream mode) and stays parked until the
  user confirms it.

**Non-Goals:**

- No daemon/proto changes in this change's planned scope; if Phase 0 shows the
  event plane missing a needed increment (e.g. feed updates only travel in
  snapshots), a minimal proto addition is scoped in a follow-up change, not
  here.
- No port of `xai-grok-pager` (product code).
- No behavioral fork of the ported crates beyond the `theway-` rename and the
  Cargo metadata (license, workspace inheritance).

## Decisions

1. **Crate naming: `xai-` → `theway-` prefix replacement.**
   `xai-grok-markdown-core` → `theway-markdown-core`,
   `xai-ratatui-textarea` → `theway-ratatui-textarea`,
   `xai-grok-pager-render` → `theway-pager-render`,
   `xai-ratatui-inline` → `theway-ratatui-inline`.
   The `grok-` segment drops with the `xai-` prefix (the resulting name
   describes the artifact, not the donor).

2. **Port = mechanical copy + rename + metadata, verified by tests.**
   Each phase copies the donor crate's `src/` into `crates/<theway-name>/`,
   rewrites `xai-`/`grok` package names and intra-repo `crate::`-level paths,
   sets `license = "Apache-2.0"` and edition/rust-version from the workspace,
   and adds a `NOTICE` file. Behavior changes land as follow-up commits in
   the same phase, never silently during the copy.

3. **License handling: Apache-2.0 in an MIT workspace.**
   Ported files keep their copyright headers. Each ported crate carries a
   `NOTICE` listing the donor (SpaceXAI, Apache-2.0), the source revision
   `5d08d7e`, and the upstream origins where the donor states them
   (ratatui-derived `terminal.rs`, tui-textarea fork, pulldown-cmark config).
   Files modified after the port get an Apache §4(b) change notice comment.
   `THIRD-PARTY-NOTICES` is not copied wholesale (762 KB of unrelated
   notices); the relevant entries are distilled into each crate's `NOTICE`.

4. **Phase 0 precedes all ports and lands independently.** It is pure theway
   code and fixes the lag the port itself would inherit. Port phases depend on
   it only for the harness (tests assert against the event-driven feed).

5. **Phase 4 gate.** `theway-ratatui-inline` is a full-screen → inline
   scroll-stream UX change. The tasks list carries it as a planning node; the
   implementation node is created only after the user confirms.

## Risks

- `xai-ratatui-textarea` uses `ratatui` feature `unstable-widget-ref` and
  `ratatui-core` 0.1 (the 0.29-line modular split). Both must resolve with the
  workspace's pinned ratatui 0.29; if the feature is not available there, the
  textarea port needs a shim or a different rendering path — checked first in
  Phase 2, with the fallback of keeping `tui-textarea` 0.7 if blocked.
- `theway-pager-render` primitives assume the pager's `SafeBuf`/block model;
  Phase 3 ports only the standalone modules (syntax, osc8, scrollbar,
  highlight, theme, line_utils) and adds theway-side adapters in
  `theway-tui`, not the pager's scrollback layout.
- syntect is a large dependency; it is only pulled in if syntax highlighting
  is selected in Phase 3 (user-visible tradeoff to confirm at that point).
