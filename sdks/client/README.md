# @theway-ai/sdk

## theway

[theway](https://github.com/ptlzc/theway) is a local, terminal-first AI agent
runtime for developer workflows. It runs inside a project as a `theway` terminal
client plus a `thewayd` daemon, keeps resumable sessions, inspects and edits
files, runs shell commands, and drives model providers including local
OpenAI-compatible servers.

Core capabilities:

- Resumable agent sessions with feed, history, slash commands, and multiple model providers.
- Filesystem and shell tools, skills, and MCP tool integration.
- Cron jobs, triggers, and local automation with a triage inbox for recurring-loop findings.
- Multi-agent DAG orchestration for running and monitoring parallel subagent graphs.
- A gRPC / HTTP+SSE+WS daemon protocol with typed SDKs for external clients.

## This package

Typed TypeScript SDK for the **theway** gRPC daemon (`theway --grpc` loopback server).
Generated from the five domain protos
([`proto/commands.proto`](proto/commands.proto),
[`proto/session.proto`](proto/session.proto),
[`proto/graph_engine.proto`](proto/graph_engine.proto),
[`proto/events.proto`](proto/events.proto),
[`proto/settings.proto`](proto/settings.proto)) + [`proto/health.proto`](proto/health.proto)
via ts-proto + `@grpc/grpc-js` — **no runtime proto loading**, no path magic.

`ThewayGrpcClient` routes each RPC to its domain service — `CommandService`,
`SessionService`, `SettingsService`, `GraphEngineService`, and `EventService` — while
`HealthClient` covers the standard `grpc.health.v1.Health` service.

Published as a public package on the official npm registry (`https://registry.npmjs.org/`).

## Install

```sh
npm install @theway-ai/sdk
```

## Documentation

The package ships protocol/spec documentation in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).
It covers session identity, `SessionSnapshot`, pagination, graph node types, streaming,
collapse, and compatibility notes.

## Usage

```ts
import { ThewayGrpcClient, HealthClient } from '@theway-ai/sdk';

const client = new ThewayGrpcClient('http://127.0.0.1:34567');

// Authoritative current session snapshot (typed SessionSnapshot)
const snapshot = await client.getSnapshot();

// Commands — throw when the server replies accepted: false
await client.prompt('hello');
await client.setModelSpec('anthropic:claude-sonnet-4-5');
await client.abort();
await client.resolveControlPlane(true);

// Graph / session control
const { resetNodeIds } = await client.graphRetry('dag-1');
const sessions = await client.listSessions();

// Daemon settings (issue #72) — partial update: only set fields are applied
const config = await client.getConfig();
await client.setConfig({ triggerPollSecs: 30, skillsDirs: ['/extra/skills'] });

// Snapshot + event stream (StreamFrame oneof: payload.$case === 'snapshot' | 'event')
await client.streamEvents((frame) => {
  if (frame.payload?.$case === 'snapshot') {
    console.log(frame.payload.snapshot.feedLines?.length);
  }
}, abortSignal);

// Standard gRPC health probe (grpc.health.v1)
const health = new HealthClient('127.0.0.1:34567');
const serving = await health.check('');

client.close();
```

## Regenerating

`crates/theway-transport/proto/` at the repo root is the single source of truth; `proto/*.proto` in this package is a mirror and
`src/generated/` is regenerated from it. Sync and regenerate with:

```sh
cd ../..                   # repo root
make hooks                 # once per clone: enable the pre-commit hook
make sdk-sync              # or: bash scripts/sdk-sync.sh
```

The enabled pre-commit hook ([`.githooks/pre-commit`](../../.githooks/pre-commit)) runs the sync automatically
whenever `crates/theway-transport/proto/` changes and stages the regenerated files into the same commit, so the checked-in proto and
generated client never drift.

Manual generation without the Makefile:

```sh
npm install
npm run gen      # protoc (grpc-tools) + ts-proto → src/generated/
```

## Publishing

Releases publish from one `vX.Y.Z` tag through the repository release workflow
([`.github/workflows/release.yml`](../../.github/workflows/release.yml)). The tag
version must equal the Cargo workspace version, every runtime crate in
[`scripts/release-crates.txt`](../../scripts/release-crates.txt), this package's
version, and `@theway-ai/plugin-sdk`'s version; [`scripts/release-validate.sh`](../../scripts/release-validate.sh)
rejects any mismatch. The workflow publishes the GitHub Release binaries, the crates.io
allowlist, and both npm SDK packages.

To cut a release, bump `version` in the root `Cargo.toml` workspace section and in both
`sdks/*/package.json` (plus their `package-lock.json` files), commit to `main`, then push
`git tag vX.Y.Z && git push origin vX.Y.Z`. No local npm login is required — GitHub Actions
uses npm trusted publishing.

## Versioning

The SDK version always equals the daemon/workspace version. Protocol contract changes
bump the shared version once, and that one version publishes every artifact.
