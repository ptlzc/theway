## Why

Today the `theway` TUI binary and the `thewayd` daemon binary each embed a full
copy of the agent runtime — harness, turn scheduler, session factory, transport
loop, snapshot builder — sharing only the low-level kernel (`kernel`/`feed`/
`listener`). Any change to turn semantics, snapshots, or command handling must
be made twice, in two implementations that drift over time (the daemon was
ported from the TUI's `web_loop` and already differs in details).

Workmate is already a pure transport client of the daemon. The TUI should be
the second client: the daemon becomes the *only* runtime, and the TUI renders
whatever the daemon's gRPC surface publishes.

## What Changes

- `theway` (TUI) becomes a **pure client**: it no longer constructs a harness,
  holds a `ReplKernel`, runs turns, or owns the session. All agent work happens
  in `thewayd`.
- `thewayd` gains the service surface a client needs: the gRPC protocol already
  has `get_state` / `stream_events` (snapshot + event frames) / `send_message` /
  `set_model` / `cancel` / `approve` / graph_*; a missing **session switch** RPC
  is added (HTTP `WebCommand::SwitchSession` exists but gRPC does not expose it).
- A new transport client crate/module (`theway-transport` client half)
  encapsulates connect + subscribe + command calls, shared by the TUI and any
  future client (workmate can adopt it later or keep its own).
- TUI startup: detect a running daemon via `<THEWAY_DIR>/daemon-port` (or the
  default port 44777) + `get_state` health probe; if absent, **spawn** `thewayd`
  (inheriting cwd/env) and wait for readiness. `-c` / `--resume-id` become
  daemon launch parameters when spawning, or a session switch against a running
  daemon.
- TUI rendering stays local: the feed is rebuilt from snapshot `feed_blocks` /
  `feed_lines`; input, scroll, history, model picker, control-plane approval all
  map to client-side UI + RPC calls. `/login`, `--list-sessions`, export/import,
  delete keep operating on the local `~/.theway` files (same machine, shared
  SQLite sessions), unchanged.
- TUI's in-process transport code is deleted: `web_loop.rs`, App's transport
  methods, kernel/feed ownership moves to daemon-only; `server::app::daemon`
  remains the single implementation.

## Capabilities

### New Capabilities
- `daemon/client`: thewayd as a local service with port discovery
  (`daemon-port`), health probe, and a first-class transport client (connect /
  subscribe / commands) that the TUI uses.

### Modified Capabilities
- `session-resource-model`: the gRPC surface gains session switching
  (`SwitchSession`) so a connected client can move the daemon to another
  session — currently only the HTTP `WebCommand` path has it.

## Impact

- `crates/theway-transport`: expose generated gRPC client + `switch_session`
  RPC; `GrpcClient` wrapper (connect, get_state, stream_events, send_message,
  set_model, cancel, approve, switch_session).
- `crates/theway-tui`: App rewritten around a client state cache (latest
  `WebStatus` + event frames); delete kernel/turn ownership, `web_loop.rs`,
  session_factory, startup assembly (moves to daemon); startup orchestrates
  daemon spawn/connect.
- `crates/theway-server`: `thewayd` gains `switch_session` handler on
  `GrpcState`; daemon keeps the full startup assembly (unchanged, now the only
  copy).
- Behavior change: tool execution cwd = daemon's cwd (daemon started in the
  target directory, or `--cwd`). TUI no longer executes tools in its own
  process.
- Tests: TUI e2e tests that drive the in-process App must be rewritten to
  spawn thewayd + client; transport client gets its own tests.
