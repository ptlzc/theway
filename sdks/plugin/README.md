# @theway-ai/plugin-sdk

TypeScript authoring SDK for theway ABI v2 runtime extensions. It supplies the public host API types, checked-in JSON Schemas, and the same `defineExtension` descriptor used by the daemon's isolated QuickJS host.

## Install

```sh
npm install --save-dev @theway-ai/plugin-sdk
```

## Create an extension

```ts
import { defineExtension } from '@theway-ai/plugin-sdk';

export default defineExtension((api) => {
  api.on('input', ({ payload }) => ({
    abiMajor: 2,
    actions: [{
      kind: 'replace_input',
      payload: { message: payload.message },
    }],
  }));
});
```

Compile the entry as an ES module and leave `@theway-ai/plugin-sdk` external. The daemon resolves that specifier to its virtual capability-brokered host module. Extension packages do not ship `node_modules` and cannot access Node globals through this SDK.

A package manifest points to the compiled entry:

```json
{
  "id": "example-extension",
  "version": "1.0.0",
  "abi": 2,
  "entry": "dist/index.js",
  "scope": "session",
  "permissions": []
}
```

The complete lifecycle, permission, registration, state, and reload contract is documented in [`docs/extensions.md`](../../docs/extensions.md). Raw ABI v2 declarations and JSON Schemas are shipped under [`abi-v2`](abi-v2).

## Validate

From the repository root:

```sh
make plugin-sdk-check
```

The check builds declarations, typechecks [`examples/basic-extension.ts`](examples/basic-extension.ts), and runs the runtime descriptor tests.
