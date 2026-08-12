## Context

Two binaries currently embed the agent runtime: `theway` (TUI) and `thewayd`
(daemon). They share low-level kernel code (`server::app::{kernel,feed,
listener,relay}`) but each owns its own transport loop, snapshot builder and
turn scheduling (`App + web_loop.rs` in the TUI vs `DaemonApp` in the server
crate). The daemon already serves workmate over gRPC (`get_state`,
`stream_events`, `send_message`, `set_model`, `cancel`, `approve`, graph_*)
and publishes its port to `<THEWAY_DIR>/daemon-port` (issue #13, phase 1).
Session storage is local SQLite under `~/.theway` shared by both processes.

## Goals / Non-Goals

**Goals:**
- The TUI becomes a pure gRPC client: no harness, no kernel, no turn
  scheduling, no session factory in its process.
- `server::app::daemon` becomes the single runtime implementation; the TUI's
  `web_loop.rs` and App transport methods are deleted.
- The TUI startup orchestrates the daemon (reuse if running, spawn if absent)
  and connects through one client wrapper.
- Local-only surfaces stay local: `/login`, `--list-sessions`, export/import,
  delete (they read `~/.theway` files directly).

**Non-Goals:**
- No `--local` in-process fallback mode (decision: single client-only form).
- No workmate frontend rewrite; it keeps its own client for now.
- No remote (non-localhost) auth/encryption story — loopback only, same as
  today.

## Decisions

1. **Transport client lives in `theway-transport`** (`client` module):
   `GrpcClient` wraps the generated proto client — connect, `get_state`,
   `stream_events` (returns a frame stream), and typed command calls. The TUI
   depends on it; workmate can migrate later. Rationale: protocol types already
   live in transport; one client implementation for all local clients.
   Alternative rejected: hand-rolled JSON over HTTP — gRPC is already the
   daemon's native surface and the TUI needs streaming.

2. **TUI state model = snapshot cache + event stream.** App keeps a
   `latest: WebStatus` cache (from `get_state` + stream snapshot frames) and
   renders the feed from `feed_blocks`/`feed_lines`. No local `Feed` mutation:
   the daemon owns the transcript. Input/scroll/history/model-picker remain
   local UI concerns. Rationale: identical rendering path for initial load and
   live updates; matches how workmate consumes the same surface.

3. **Session selection is a daemon launch concern.** `theway -c` /
   `--resume-id` / bare start map to daemon launch args when the TUI spawns the
   daemon (`thewayd --resume-id ...`), and to `SwitchSession` RPC when a daemon
   is already running. The daemon keeps "one process = one active session".

4. **Daemon lifecycle: detached, not TUI-owned.** The TUI does not kill the
   daemon on exit (multi-client sharing); `Ctrl-C`/SIGTERM on the daemon stops
   it. The TUI shows a reconnect/offline state when the stream drops.

5. **gRPC session switch**: add `switch_session(Request<String>) ->
   CommandResult` on `ThewayGrpc` (mirror of the HTTP `WebCommand::SwitchSession`
   path in `DaemonApp::handle_web_command`). Shared handler stays in
   `DaemonApp`.

6. **`/login` stays local**: writes `~/.theway/auth.json` from the TUI process
   (same machine, same user). The daemon picks it up on the next turn via the
   auth-store stream fn. No protocol change.

7. **Deleted from the TUI**: `web_loop.rs`, `App::{transport_endpoints,
   run_transport_loop, web_snapshot, ...}` transport methods, kernel/feed
   ownership, `session_factory` module, startup assembly (harness/session/
   triggers/DAG/MCP construction) — all of it already lives in the daemon.

## Risks / Trade-offs

- **Tool cwd semantics change**: tools execute in the daemon's cwd (daemon
  started in the target directory or `--cwd`). Users must start the daemon in
  the project directory; the TUI can no longer "run here" independently. This
  is the intended single-form behavior but is the biggest user-visible change.
- **Event-loop rewrite risk**: the TUI's serialized turn loop is replaced by a
  network-driven loop; in-flight-turn UI (abort, queued messages, busy state)
  now depends on daemon round-trips. Localhost latency is negligible, but
  disconnect handling (daemon died mid-turn) must be explicit.
- **Test surface**: e2e tests that drive the in-process App must spawn
  `thewayd` + client instead; client tests added in transport. Expect a large
  test-diff in this change.
- **TUI-only interactions**: clipboard paste goes through `send_message` image
  payloads (already supported); `/web-connect` relay stays a daemon feature
  (TUI delegates or the command reports it is daemon-side).
