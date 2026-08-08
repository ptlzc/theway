# `@file` mentions + interactive session picker

> Parent: master roadmap issue.
> Tier: 1 (daily UX).

## Goal

Two related affordances that both need fuzzy completion inside the input box:

1. **`@file` mention**: typing `@<prefix>` opens a completion popup of repo-rooted file paths.
   Selecting one inserts a token like `@src/foo/bar.rs`. On submit, the agent prepends a
   "Files in context:" block to the user message that includes the file content
   (size-capped).
2. **`/sessions` or `--pick`**: an interactive picker that lists prior sessions for the cwd
   (preview, age, message count) and lets the user resume with `Enter`.

## Architecture

```
theway/src/picker/
  mod.rs          Picker trait + Item trait (display, score)
  fuzzy.rs        SkimMatcher v2 wrapper
  files.rs        repo file walker (gitignore-aware, cached, debounced)
  sessions.rs     SessionEntry → Item adapter
  ui.rs           ratatui-free renderer that draws into the input region overlay
```

File mentions hook into the input editor:

- Detect `@<prefix>` as you type; ask `files::candidates(prefix)` for top 20.
- On submit, scan the raw input for `@token`s, resolve each, read the file (size-capped,
  e.g. 200 KiB), and synthesize an attachment block injected before the prompt text:
  `<file path="src/foo.rs">…content…</file>`.

Session picker lives behind `/sessions` (interactive) and a one-shot `theway --pick` flag for
shell users who want to resume without typing an id.

## Stability

- File walker must not block the input loop. It runs as a background tokio task with a debounce
  (50ms). If indexing is in progress, completion uses what's been indexed so far.
- Gitignore-aware via `ignore` crate (same as ripgrep). Skips `.git`, `target`, `node_modules`
  by default. (`.thewayignore` is intentionally not supported — see master non-goals.)
- File-content injection has a hard size limit. If exceeded, the injection becomes a stub
  (path + size + first N lines) so the LLM still knows it exists.
- Picker is cancellable via Esc (no resume happens).

## Extensibility

- The `Picker` trait powers other fuzzy menus that come later (theme picker, skill picker, MCP
  server picker).
- `Item` is generic so each menu has its own preview rendering.
- File-content injection format is a single helper so the LLM-prompt format can evolve in one
  place.

## Performance

- File index is in-memory `Arc<Vec<PathBuf>>`, rebuilt on filesystem-watcher events (via
  `notify` crate) with a 250ms debounce. For large repos (≥ 100k files) the initial index runs
  in a worker; mention completion falls back to a small linear scan until ready.
- Fuzzy match scoring is bounded — top-N early-exit.

## Testing

| Layer | What |
|---|---|
| **unit** | Fuzzy ranking on a fixture; gitignore exclusions; size-cap truncation produces stub. |
| **integration** | Submit a prompt with `@src/foo.rs` → resulting `AgentMessage` user content contains a `<file>` block with foo.rs contents. |
| **e2e** | PTY: type `@li` in a tempdir with `lib.rs`, expect popup; arrow-down + Enter inserts the token; submit; verify the assistant request includes the file content (captured via faux provider stub). `/sessions` opens, arrow keys, Enter resumes a real session. |

## Acceptance criteria

- Typing `@` opens a completion popup within 50ms even in a 50k-file repo (degraded mode is
  fine, but no freeze).
- A 1 MiB file mentioned via `@` becomes a truncated stub, not the full payload.
- `.gitignore` and `.thewayignore` exclusions apply.
- `/sessions` picker shows preview of the first user prompt per session, sorted by recency.
- `Enter` in the picker resumes via the same code path as `--resume-id`.

## Out of scope

- `@symbol` (function/struct name) lookup — needs LSP, see [[11-builtin-capabilities]].
- Image attachments via `@image.png` — see [[15-windows-multimodal]].
