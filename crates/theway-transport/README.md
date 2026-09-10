# theway-transport

English | [中文](README.zh.md)

`theway-transport` owns the cross-client wire model and the gRPC and web transports used to control a theway daemon. It provides generated protobuf services, HTTP JSON-RPC, server-sent events, WebSocket events, a typed gRPC client, daemon discovery helpers, and transport-facing operation traits.

The crate is independent of `theway-core`, `theway-daemon`, and `theway-storage`. A server implements `TransportHost` and supplies `TransportEndpoints`; clients use wire/protobuf types and `GrpcClient` without accessing runtime internals.

## Protocol entry points

- `wire` defines commands, complete and incremental status snapshots, graph/job events, configuration, operation request/result records, and open runtime-extension catalog, diagnostic, command, contribution, trust, and reload records.
- `transport` defines `TransportEndpoints` plus `SessionOps`, `JobOps`, `GraphOps`, `ToolOps`, and `StorageOps`.
- `grpc`, `http`, and `ws` expose those operations over protobuf RPC, JSON-RPC, SSE, and WebSocket connections.
- `proto`, `tools`, and `state` convert between internal wire records and generated protobuf messages.
- `client` wraps tonic clients and discovers or starts a per-working-directory daemon.
- Shared modules such as `feed`, `commands`, `auth`, `history`, `images`, and `mentions` define client/daemon data that is not tied to a particular carrier.

MCP transport is not implemented here: external MCP clients live in `theway-mcp`, and the daemon's MCP server lives in `theway-daemon`.

## Feed blocks and transcript replay

`WireFeedBlock::User` carries the round's own text plus `attachments: Vec<WireFeedAttachment>` and `source: Option<WireFeedSource>`: an attachment is `{ kind: "file" | "image", name, detail }` with a display-only `detail`, and a source is `{ kind: "user" | "trigger" | "subagent" | "host", label }`. `WireFeedBlock::Context { label, text, timestamp }` is the separate row for content injected before the model saw the round. The added fields are optional on the wire, so a payload written without them still deserializes.

`TranscriptEntry::{Message, UserInput}` and `replay_entries` rebuild a stored transcript from either source: a `user_input` record supplies the user block — submitted text, one chip per `File` / `Image` part, origin — and one `Context` row per `Injected` part, after which the `Message::User` the record describes is skipped. `user_input_blocks` is the pure record-to-blocks mapping, and `replay_messages` rebuilds transcripts that hold messages only. A context row renders as `[<label>] <text>` — the bare text when the label is empty — with embedded newlines flattened to spaces; `feed::plain::context_line` is the single implementation of that row for the plain projection, the plain-lines cache, and the TUI's styled renderer.

`proto/session.proto` mirrors the shapes: `FeedBlock.context = 9`, `UserBlock.attachments = 3` and `UserBlock.source = 4`, `FeedAttachment.kind = 1` / `name = 2` / `detail = 3`, `FeedSource.kind = 1` / `label = 2`, and `ContextBlock.label = 1` / `text = 2` / `timestamp = 3`.

`mentions::mentions` is the single `@path` parser: the daemon resolves a round's mentions once at admission and records the files it read, so no client expands mentions. `mentions::expand` is the text-path helper that reads each mention and appends a `Files in context:` block to the prompt text.

## Skill prompt envelope

`attach_skill_prompt(text, Some(name))` renders the skill envelope from `SKILL_PROMPT_LEAD` and `SKILL_PROMPT_USER_MARKER` — the two constants that are the format's single definition — and passes `text` through unchanged when no skill name is given. `skill_prompt_preamble(name)` renders the preamble alone, and `split_skill_prompt` is the inverse that parses an envelope back into the skill name and the user's own text, returning `None` for a prompt that is not one. The daemon's input admission splits a submitted envelope with `split_skill_prompt`, so the format must stay round-trippable: `split_skill_prompt(&attach_skill_prompt(text, Some(name)))` returns the name and the exact text the builder wrapped.

## Documentation

- [Wire and transport architecture](docs/architecture.md)

## Validation

```bash
cargo test -p theway-transport
cargo doc -p theway-transport --no-deps --document-private-items
make layering-check
```
