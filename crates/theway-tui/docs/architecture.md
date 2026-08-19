# theway-tui architecture

English | [中文](architecture.zh.md)

## Ownership and dependency direction

`theway-tui` owns the terminal client form and controller-local resources. It speaks only records and services from `theway-transport` and uses `theway-storage` for controller-local persistence. It does not import the agent runtime or daemon application crates.

Client-specific behavior includes terminal layout, keyboard and mouse handling, feed rendering, local pickers and commands, clipboard images, daemon attachment defaults, and how local files/processes are exposed through the controller services. Cross-client runtime behavior starts in transport records and is implemented by the daemon.

## Command dispatch

[`main.rs`](../src/main.rs) separates commands before starting the terminal application. Session export/import and standalone maintenance can open a local repository directly. Interactive use delegates to [`startup/mod.rs`](../src/startup/mod.rs).

[`cli/mod.rs`](../src/cli/mod.rs) owns argument parsing and offline session command behavior. [`config_payload.rs`](../src/config_payload.rs) assembles local configuration plus CLI overrides into `WireDaemonConfig` and provisions the running daemon through the settings RPC.

## Controller startup

Interactive startup performs these operations in order:

1. Start a loopback tool service backed by [`local_tool_ops.rs`](../src/local_tool_ops.rs), rooted at the selected working directory.
2. Start a loopback storage service backed by [`controller_storage.rs`](../src/controller_storage.rs) and a local session repository.
3. Put both service addresses into the daemon configuration payload.
4. Discover a compatible daemon through the per-working-directory port file or default port, or spawn `thewayd` and wait for readiness.
5. Provision configuration, fetch the initial snapshot, apply client-owned fresh/resume selection, and construct `ui::App`.

`LocalToolOps` implements transport `ToolOps` for file read/write/edit, command execution, directory/search operations, memory, and skill installation. `ControllerSessionOps` and `ControllerStorageOps` implement session lifecycle and DAG/trigger/cron persistence. These implementations are controller policy; they do not turn the TUI into an agent runtime.

## Application state and events

[`ui/mod.rs`](../src/ui/mod.rs) owns `App`, presentation state, overlays, selection, scroll state, composer state, and the latest transport snapshot. [`ui/app/event_loop.rs`](../src/ui/app/event_loop.rs) and its sibling modules split event polling, frame application, rendering, interaction, panels, status, and headless output without creating additional ownership layers.

[`ui/app/snapshot.rs`](../src/ui/app/snapshot.rs) applies complete snapshots and incremental stream frames. A session-id change resets session-scoped presentation caches. Feed deltas are accepted only against the expected base; a complete snapshot is the recovery path after mismatch or lag.

User actions call typed `GrpcClient` methods or enqueue transport commands. The UI never calls `AgentHarness`, graph-engine internals, or daemon-private services.

## Feed and composer rendering

[`feed_cache.rs`](../src/feed_cache.rs) caches rendered feed lines, maintains a bounded window, and incrementally renders append-only assistant/thinking blocks while falling back to a full render after non-append edits. [`feed_render.rs`](../src/feed_render.rs) maps transport feed blocks to themed ratatui lines, code-block spans, and link overlays.

[`ui/app_input.rs`](../src/ui/app_input.rs) and [`ui/app_input/history.rs`](../src/ui/app_input/history.rs) own composer input, completion, history, paste, and submission. The editor state comes from `theway-ratatui-textarea`; terminal rendering helpers and link/scrollbar behavior come from `theway-pager-render`.

[`theme.rs`](../src/theme.rs) is the terminal appearance owner. Color, spacing, prefixes, loading indicators, and panel layout do not enter daemon snapshots or core events.

## Offline persistence

Offline session export, import, list, and delete use `theway-storage` directly because they operate on local artifacts outside an active turn. Interactive session creation, switching, rename, and deletion use protocol operations so daemon and client state remain serialized.

## Invariants

- The crate never depends on `theway-core` or `theway-daemon`.
- Controller tool and storage services bind to loopback addresses and are provisioned explicitly to the daemon.
- Runtime state comes from transport snapshots and events; UI caches are derived and resettable.
- Client appearance and input choices do not enter shared wire types unless another client needs the same behavior.
- Interactive session mutations use transport operations; direct SQLite access is limited to controller storage implementation and offline maintenance.
- Streaming and one-shot feed rendering converge to the same visible content and source/link mapping.
