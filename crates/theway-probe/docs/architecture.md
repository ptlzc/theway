# theway-probe architecture

English | [中文](architecture.zh.md)

## Build-time protocol ownership

[`build.rs`](../build.rs) reads the protobuf sources owned by `theway-transport`, including `commands.proto` and `health.proto`, compiles the command, session, graph, event, and health definitions with `protox`, and generates tonic clients with `tonic-prost-build`. The probe deliberately has no Rust dependency on `theway-transport`; it verifies the external gRPC service as an independent consumer.

Any protobuf change that affects the imported services must leave this build script able to compile the complete import graph. The transport crate remains the only source of proto definitions.

## Runtime flow

[`main.rs`](../src/main.rs) parses the server address, selected checks, and optional output directory. It executes selected checks sequentially so results and server mutations are deterministic:

- `health-check` calls `grpc.health.v1.Health/Check` and requires `SERVING`.
- `health-watch` calls `Health/Watch` and requires a serving update from the stream.
- `multi-session` creates sessions and confirms the session service can list independently addressable ids.
- `get-state` calls the command service and verifies a session snapshot can be retrieved.

Each check returns a `TestResult` with evidence and optional failure detail. The command prints every result, writes JSON files when requested, and exits with status 1 when the passed count differs from the selected count.

## Invariants

- The probe connects only to the address supplied by the operator and never starts a daemon.
- Tests remain provider-independent and do not submit prompts that require an API key.
- A selected check produces one terminal result; unknown check names fail explicitly.
- Protobuf definitions are consumed from `theway-transport` rather than copied into this crate.
- Process exit status reflects the aggregate selected-check result.
