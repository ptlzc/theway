# @theway-ai/sdk

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

Published to the internal Nexus npm-private registry (`https://registry.npmjs.org/repository/npm-private/`).

## Install

```sh
npm install @theway-ai/sdk
```

## Usage

```ts
import { ThewayGrpcClient, HealthClient } from '@theway-ai/sdk';

const client = new ThewayGrpcClient('http://127.0.0.1:34567');

// Full session state (typed SessionState)
const state = await client.getState();

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

Publishing to the Nexus registry is manual ([`scripts/sdk-publish.sh`](../../scripts/sdk-publish.sh)):

```sh
cd ../..
make sdk-publish BUMP=minor ISSUE="#62"
# 或: bash scripts/sdk-publish.sh minor "#62"
```

The script verifies the tree is clean and in sync, bumps the version, runs `npm publish` against the
registry in `publishConfig`, and commits the version bump. The publishing machine's `~/.npmrc` must hold a
`_authToken` for `//registry.npmjs.org/repository/npm-private/`.

## Versioning

The SDK version tracks the theway release that the proto contract was cut from
(theway v1.0.0 → SDK 1.0.0). Bump together with contract changes.
