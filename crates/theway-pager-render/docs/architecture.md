# Pager rendering architecture

English | [中文](architecture.zh.md)

## Responsibility

`theway-pager-render` is a presentation utility layer below `theway-tui`. Its functions operate on ratatui buffers, styled lines, scroll geometry, URLs, and paths; the caller owns application state, input handling, navigation, and target activation.

## Text and geometry

[`line_utils.rs`](../src/line_utils.rs) centralizes terminal-display-width operations over ratatui lines. Slicing and truncation preserve span styles and respect Unicode grapheme boundaries, so callers do not substitute UTF-8 byte length for terminal columns.

[`scrollbar.rs`](../src/scrollbar.rs) converts content length, viewport length, and scroll position into scrollbar rendering. [`color.rs`](../src/color.rs) contains buffer-level color blending and clearing helpers without choosing application themes.

## Link and path annotation

[`osc8.rs`](../src/osc8.rs) detects URL and file-like targets in rendered lines and adds OSC 8 link metadata. Network URLs are restricted to `http` and `https`; arbitrary URI schemes are not promoted to clickable targets. The caller decides whether and how an annotated target is opened.

[`tool_paths.rs`](../src/tool_paths.rs) resolves tool-reported paths against an explicit working directory and produces compact display forms. Resolution must not depend on a hidden process working directory when the caller can supply the relevant base path.

## Boundaries and invariants

- Utility functions do not own feed, selection, session, transport, or daemon state.
- Visible-column calculations are grapheme-aware and use terminal display width.
- Link detection does not execute a target and does not accept unrestricted URI schemes.
- Path helpers preserve the resolved target separately from shortened display text.
