# @theway-ai/sdk

Typed TypeScript SDK for the **theway** gRPC daemon (`theway --grpc` loopback server).
Generated from [`proto/theway_grpc.proto`](proto/theway_grpc.proto) (service entrypoint
plus its domain imports) + [`proto/health.proto`](proto/health.proto) via ts-proto +
`@grpc/grpc-js` — **no runtime proto loading**, no path magic.

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

```sh
npm install
npm run gen      # protoc (grpc-tools) + ts-proto → src/generated/
```

## Versioning

The SDK version tracks the theway release that the proto contract was cut from
(theway v1.0.0 → SDK 1.0.0). Bump together with contract changes.
