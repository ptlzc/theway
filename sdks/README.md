# theway SDKs

This directory contains independently versioned SDK packages:

- [`client`](client) — `@theway-ai/sdk`, the typed gRPC daemon client.
- [`plugin`](plugin) — `@theway-ai/plugin-sdk`, the runtime-extension authoring SDK and its single unversioned ABI.

Run `make sdks-check` from the repository root to build and validate both packages.
