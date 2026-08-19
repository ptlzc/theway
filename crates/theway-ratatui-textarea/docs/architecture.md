# Textarea architecture

English | [中文](architecture.zh.md)

## Responsibility

`theway-ratatui-textarea` owns reusable text-editing and widget behavior. The embedding application supplies the surrounding form, command routing, clipboard implementation, theme, and meaning of atomic text-element events.

## Edit engine

[`editor.rs`](../src/editor.rs) contains `EditBuffer`, command classification, edit planning, and validated edit application. Cursor positions and replacement ranges are normalized to Unicode grapheme boundaries. Atomic byte ranges are also normalized before planning so cursor movement and deletion cannot split an application-defined element.

An `EditPlan` captures its buffer identity and generation at creation. Applying a stale or foreign plan fails instead of mutating a different text state. Edit results carry both text deltas and cursor outcomes so higher layers can update dependent ranges explicitly.

## Widget state and interaction

[`textarea.rs`](../src/textarea.rs) exposes the widget and its state. [`textarea/model.rs`](../src/textarea/model.rs), [`textarea/navigation.rs`](../src/textarea/navigation.rs), [`textarea/mouse.rs`](../src/textarea/mouse.rs), [`textarea/elements_wrap.rs`](../src/textarea/elements_wrap.rs), and [`textarea/history.rs`](../src/textarea/history.rs) separate those mechanisms while retaining one public ownership boundary.

[`textarea/history.rs`](../src/textarea/history.rs) records undo and redo states and supports grouping edits that should behave as one user action. [`textarea/mouse.rs`](../src/textarea/mouse.rs) maps terminal coordinates to text positions and selection actions. Clipboard operations use the caller-provided `ClipboardProvider` or the internal fallback.

## Wrapping and rendering

[`wrapping.rs`](../src/wrapping.rs) maps logical text and styled spans into visual rows while preserving grapheme boundaries, terminal display width, and source positions. [`render/mod.rs`](../src/render/mod.rs) draws content, selection, cursor, and scrollbar from the state without changing the edit model.

## Boundaries and invariants

- Cursor and edit boundaries are UTF-8 byte offsets that always land on grapheme boundaries.
- Atomic text elements move, select, and delete as indivisible ranges.
- A plan can mutate only the buffer identity and generation from which it was created.
- Wrapping preserves styles and maps visual cells back to logical positions using terminal display width.
- On Windows, Ctrl+Alt input is distinguished as AltGr where required; other platforms use their composed input behavior.
