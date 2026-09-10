# theway-transport architecture

English | [中文](architecture.zh.md)

## Dependency position

`theway-transport` depends on `theway-contract` for shared persisted records and on `theway-llm-provider` for model-facing data reused in configuration and snapshots. It does not depend on the runtime engine, daemon application, or storage implementation.

This direction lets any server or client use the protocol without linking `AgentHarness`, SQLite, terminal rendering, or daemon-owned tools.

## Wire model and endpoint API

[`wire.rs`](../src/wire.rs) is the serde representation shared by the server event loop and JSON transports. `WireCommand` carries mutations into the serialized runtime loop, including the atomic `ActivateSession` command and write-only `SetCredential` / `ClearCredential` commands so they are ordered with turns. `WireStatus` is the authoritative client snapshot, while `WireStatusUpdate` can carry a complete snapshot or a feed delta that applies only when its base indexes match the receiving snapshot.

[`transport.rs`](../src/transport.rs) defines the server-facing API:

- `TransportEndpoints` contains the command channel, status broadcaster and latest snapshot, agent and DAG event broadcasters, session identity and path/config views, plus operation trait objects.
- `SessionOps`, `JobOps`, `GraphOps`, `ToolOps`, and `StorageOps` expose request/response operations that do not need direct runtime state access.
- `Unavailable*` implementations provide explicit errors or empty behavior when a host does not support an optional operation group.

[`host.rs`](../src/host.rs) defines `TransportHost`. The server gives a transport its endpoints, then runs the serialized application loop alongside the server task. The concrete implementation is daemon-owned.

## gRPC carrier

The protobuf files, including [`commands.proto`](../proto/commands.proto), [`events.proto`](../proto/events.proto), and [`state.proto`](../proto/state.proto), are the source of truth for services and messages. [`build.rs`](../build.rs) compiles every proto file with `protox` and `tonic-prost-build`, so no system `protoc` is required.

[`grpc/mod.rs`](../src/grpc/mod.rs) maps command, session, settings, graph, event, tool, storage, and health services to `TransportEndpoints`. Mutating operations that must serialize with turns enqueue a `WireCommand`; read/control traits execute through their endpoint object. The session service exposes `ActivateSession`, `SetCredential`, and `ClearCredential`; credential secrets are write-only and never appear in responses, snapshots, or events. Event subscriptions receive current state and then incremental frames, with lag recovery from the authoritative latest snapshot.

[`proto.rs`](../src/proto.rs), [`tools.rs`](../src/tools.rs), and [`state.rs`](../src/state.rs) own protobuf conversion for session state, tool operations, and runtime storage records. A proto change must update these conversions and the generated TypeScript SDK through `make sdk-sync`.

## Web carriers

[`http.rs`](../src/http.rs) serves health, JSON-RPC, SSE events, and WebSocket upgrade routes from the same endpoint set used by gRPC. [`ws.rs`](../src/ws.rs) accepts JSON commands and publishes status, agent, and DAG events as JSON frames.

HTTP and WebSocket handlers translate carrier input into transport-owned requests or `WireCommand` values. They do not implement model, session, storage, tool, or graph policy.

## Client and daemon discovery

[`client.rs`](../src/client.rs) wraps generated tonic clients in `GrpcClient` and exposes typed commands, state streams, session/graph control, controller tool service, and storage service calls.

Daemon discovery reads a per-working-directory port/pid file under `${THEWAY_DIR:-$HOME/.theway}`, probes candidate loopback addresses, removes stale entries only when ownership matches, and can spawn `thewayd` before waiting for readiness. Discovery is loopback-oriented and does not add an authentication protocol.

## Shared client records

Modules including [`feed/mod.rs`](../src/feed/mod.rs), [`commands.rs`](../src/commands.rs), [`auth.rs`](../src/auth.rs), [`history.rs`](../src/history.rs), [`images.rs`](../src/images.rs), and [`mentions.rs`](../src/mentions.rs) define reusable client/daemon records and pure helpers. Leaf path, trigger, cron, and raw persistence definitions remain in `theway-contract` and are re-exported only where a stable transport path requires it.

## Feed replay and structured input records

[`feed/wire.rs`](../src/feed/wire.rs) owns the serde feed-block enum, [`feed/types.rs`](../src/feed/types.rs) mirrors it as the `Block` model, and [`feed/model.rs`](../src/feed/model.rs) owns the structured `Feed` shared by the producing daemon and the consuming clients. The user block carries attachment chips and an origin next to its text, and the context row is a block of its own.

[`feed/replay.rs`](../src/feed/replay.rs) rebuilds blocks from stored data: `replay_entries` consumes `TranscriptEntry::{Message, UserInput}`, so a session that carries structured records replays the user block and its context rows from the record, while `replay_messages` serves transcripts that hold messages only. A record describes the `Message::User` entry written immediately after it and replay skips exactly that entry, so one round renders once.

[`feed/plain.rs`](../src/feed/plain.rs) owns the display form of a context row: `context_line` renders `[<label>] <text>` and flattens embedded newlines, and `Feed::plain_lines`, the plain-lines cache, and the TUI's styled renderer all call it, so one context block stays one counted row.

[`proto/feed.rs`](../src/proto/feed.rs) converts generated `FeedBlock` messages into wire blocks and [`proto/resources.rs`](../src/proto/resources.rs) converts wire blocks back, covering the attachment chips, the origin, and the context row. Proto source, both conversion directions, and the generated SDK output change together.

## Skill prompt envelope

[`commands.rs`](../src/commands.rs) owns the skill envelope: the wrapper that makes an agent invoke a named skill before answering a turn. `SKILL_PROMPT_LEAD` and `SKILL_PROMPT_USER_MARKER` are the format's single definition — `attach_skill_prompt` renders them around the skill name and the user's own text, and passes the text through when no name is given — so the builder, the preamble, and the parser cannot drift apart.

`skill_prompt_preamble` renders the same prefix alone as the content injected ahead of the turn, and `split_skill_prompt` parses a prompt back into its `(skill name, user text)` pair or returns `None` when the prompt is not an envelope. The daemon's input admission splits a `/skill` turn with `split_skill_prompt` to record the user's own text, so the format stays round-trippable: `split_skill_prompt(&attach_skill_prompt(text, Some(name)))` returns `(name, text)`, and no caller re-derives the format from the rendered string.

## Invariants

- Wire and protobuf records never contain core or daemon-private types.
- All carriers drive the same `TransportEndpoints` semantics; carrier-specific handlers do not acquire business policy.
- Runtime mutations that need ordering enter the serialized command queue.
- Snapshot deltas apply only to the matching base and recover through a complete authoritative snapshot after lag or mismatch.
- Proto source, Rust conversions, service handlers, client calls, and generated SDK output change together.
- A feed user block comes from the structured input record when the transcript carries one, and from the stored message otherwise; both paths emit the same block kinds.
- The skill envelope keeps one definition: `split_skill_prompt` recovers the name and text `attach_skill_prompt` wrapped, and `skill_prompt_preamble` renders the same prefix.
- The crate contains no client appearance, terminal input handling, storage backend, or MCP implementation.
