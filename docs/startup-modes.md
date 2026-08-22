# Startup modes

English | [中文](startup-modes.zh.md)

This document describes how `theway` and `thewayd` are started and how their lifetimes relate.

## Processes

- `theway` is the terminal client/controller.
- `thewayd` is the headless daemon that owns the agent runtime.
- The client discovers a running daemon through the per-cwd port file, reuses it when present, or spawns one.

## Two lifecycle modes

### Attached mode (default when `theway` spawns a daemon)

When `theway` starts and no daemon is available, it starts controller services (tool service and storage service) and spawns `thewayd` with the controller storage address.

The daemon is controller-backed: it probes the controller storage service and shuts down when that service disappears.

The result is that the daemon lifetime is tied to the TUI. When the TUI exits, the daemon stops shortly afterward.

### Standalone background mode

When `thewayd` is started manually without the controller storage address, it uses local storage and runs independently.

It keeps running after any client exits, and later `theway` runs can discover and reuse it.

This is the mode to use for a long-lived background daemon.

## CLI: `theway --daemon`

The `--daemon` flag on `theway` makes the background mode explicit from the client:

- `theway` (default): spawn an attached daemon that shares the TUI lifecycle.
- `theway --daemon`: spawn a standalone background daemon with an independent lifecycle. The current TUI connects to it, and future TUI runs reuse it.

Under the hood, `theway --daemon` spawns `thewayd` without the controller storage address and detaches the daemon process so it survives terminal session close.

## Implementation notes

- Keep `thewayd` standalone by default for manual use.
- The current `--storage-service-addr` flag remains the internal/wire mechanism for controller-backed storage, but the user-facing lifecycle switch belongs on `theway`.
- For true background persistence after terminal or SSH session close, the spawned daemon should be detached:
  - Unix: use `setsid`, ignore `SIGHUP`, and redirect standard streams.
  - Windows: use `DETACHED_PROCESS` or `CREATE_NEW_PROCESS_GROUP` (or equivalent).

## Mode comparison

| Mode | Command | Daemon storage | TUI exit | Next TUI |
|---|---|---|---|---|
| Attached | `theway` | controller-backed | daemon stops | spawns a new daemon |
| Standalone manual | `thewayd --cwd ...` | local | daemon keeps running | reuses daemon |
| Standalone via client | `theway --daemon` | local | daemon keeps running | reuses daemon |
