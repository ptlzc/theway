# theway-tui

English | [中文](README.zh.md)

`theway-tui` builds the `theway` command: a ratatui client/controller for `thewayd` plus offline session-maintenance commands. It owns terminal layout, input, feed rendering, pickers, clipboard integration, local command presentation, daemon discovery/spawn, and the controller-side tool and storage services used by a connected daemon.

The crate depends on `theway-transport`, `theway-storage`, and rendering widgets, but never on `theway-core` or `theway-daemon`. Runtime turns, triggers, tools exposed to the model, and orchestration state remain daemon-owned.

## Runtime modes

- Interactive mode starts loopback `ToolService` and `StorageService` implementations, discovers or spawns `thewayd`, provisions daemon configuration, consumes snapshots/events, and runs the terminal application.
- Offline session commands open the local `SqliteSessionRepo` directly for export, import, listing, and deletion when no live runtime coordination is required.
- Headless/non-interactive rendering reuses the same application state and transport frames without constructing an agent runtime.

## UI building blocks

- `theway-markdown` renders streaming assistant content, code, math, tables, links, and Mermaid diagrams.
- `theway-ratatui-textarea` provides the composer editor.
- `theway-pager-render` provides width, scrollbar, color, path, and OSC 8 link helpers.

## Documentation

- [Client/controller architecture](docs/architecture.md)

## Validation

```bash
cargo test -p theway-tui
cargo doc -p theway-tui --no-deps --document-private-items
make layering-check
```
