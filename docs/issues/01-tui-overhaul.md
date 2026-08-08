# TUI overhaul: input box, multi-line, history, mid-stream abort, spinner, streaming markdown

> Parent: master roadmap issue.
> Tier: 1 (daily UX).
> Owner: TBD.

## Goal

The REPL is line-based with `stdin().lock().read_line` and prints output through `crossterm`
without raw mode. Streaming assistant output interleaves with the user's in-progress input,
Ctrl-C kills the process, there is no spinner, and pasting a multi-line snippet looks broken.

After this issue:

- Bottom-fixed input box that survives streaming output above.
- Multi-line input with `Shift-Enter`, Emacs-style keybinds (`Ctrl-A/E/U/K/W`, `Alt-←/→`).
- Persistent input history with `↑/↓` recall and `Ctrl-R` reverse search.
- `Ctrl-C` aborts the current turn cleanly (token cancellation, tool kill) and keeps the REPL
  alive; second `Ctrl-C` within 1.5s exits.
- Animated spinner with current phase (e.g. `thinking…`, `tool: bash 2.4s`).
- Streaming Markdown rendering: code blocks get syntax highlighting on the fly, lists/headings
  render incrementally without rewriting earlier lines.
- Bracketed-paste detection: pasting multi-line text never auto-submits.

## Architecture

A new `theway-tui` Rust crate is **out of scope** (we tried and decided no); instead we layer on
top of `crossterm` raw mode with a custom split-view renderer in `theway::tui`.

```
theway/src/tui/
  mod.rs          Tui facade (banner, errors, listener)
  layout.rs       split-view renderer — output region (scroll) + input region (sticky)
  input.rs        editor + history + bracketed paste + keybind table
  spinner.rs      throttled redraw loop, named phases
  markdown.rs     streaming-friendly Markdown → ANSI renderer
  abort.rs        Ctrl-C → CancellationToken handoff
```

Listener flow stays the same — `AgentEvent` → `tui::handle_event` — but renderer routes output
to the **scrollback region** instead of stdout-as-stream. Input region redraws on every
keystroke. Spinner is a separate tokio task throttled to ≤ 30 fps.

`Ctrl-C` flips the active `CancellationToken` held by `Agent`, which propagates through
streaming + every tool. The REPL loop catches the abort, prints `[aborted]`, and reissues the
prompt marker.

## Stability

- Raw mode must restore on every exit path. Wrap startup in a `Drop` guard. `panic = "unwind"`
  default keeps the destructor running on panic.
- Spinner task holds **no** locks. It reads an `Arc<atomic>` phase tag only.
- Redraw is single-threaded via a `tokio::sync::Mutex<Renderer>`. Output writer is buffered
  and flushed once per frame, not per event.
- Resize: SIGWINCH → recompute layout → repaint scrollback tail + input. Test with
  `vt100`/`portable-pty` fixture in CI.

## Extensibility

- Keybindings live in a `Keymap` struct constructable from user config later (`~/.theway/keybindings.toml`).
  v1 ships with hard defaults; we just don't bake the literals into the input loop.
- Markdown renderer is a trait so future themes / disabled-color modes plug in.
- Spinner phase tag is `enum SpinnerPhase { Idle, Thinking, Tool(String), Persisting }` —
  upstream emitters use the enum, not free-form strings.

## Performance

- Repaint rate capped at 30 fps. Aggregate streaming `text_delta` events between frames.
- Input editor must stay responsive under heavy output: keystroke handling cannot block on the
  rendering task — they communicate via a bounded `tokio::sync::mpsc` (buffer 256).
- Markdown rendering for code blocks is incremental: highlight only the new tail lines per
  frame, not the full block.
- Memory: scrollback ring is a fixed-size `VecDeque<Line>` (default 5_000 lines). Cap is
  configurable, default fixed.

## Testing

| Layer | What |
|---|---|
| **unit** | Keymap parsing; spinner phase transitions; markdown streaming chunker handles split-mid-token cases; abort token propagation timing. |
| **integration** | `vt100::Parser` headless terminal: send Ctrl-C mid-stream, assert abort emitted and REPL prompt restored. Send `↑` after two prompts, assert recall. Paste multi-line via bracketed-paste, assert no auto-submit. |
| **e2e** | `portable-pty`-driven `theway` binary with the faux provider script: simulate a 5-second streaming response, send Ctrl-C at 2s, assert process is still alive + REPL prompt + session jsonl still well-formed. |

CI matrix: ubuntu-latest + macos-latest. Skip windows on this issue (separate issue).

## Acceptance criteria

- Mid-stream Ctrl-C cancels the turn, does NOT kill the process, and leaves the session jsonl
  consistent (active leaf points at the user prompt; no half-written assistant entry).
- Pasting a 50-line code block into the input box does not submit until Enter is pressed
  outside paste mode.
- After two prompts, `↑` recalls the most recent and `Ctrl-R` finds matches by substring.
- During a streaming response, the input box does NOT scroll or get overwritten.
- A code block in assistant output is syntax-highlighted within ≤ 1 frame of the closing
  triple-backtick streaming through.
- Resizing the terminal during a stream does not corrupt scrollback or input.

## Out of scope

- Themes / colour schemes (separate issue under Tier 6).
- Image rendering in the terminal (Tier 7).
- Persisted keybindings config (called out but defer to a follow-up under settings-manager).
