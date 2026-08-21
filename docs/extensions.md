# Runtime extensions (ABI v2)

English | [中文](extensions.zh.md)

Runtime extensions are capability-scoped JavaScript or TypeScript packages executed by the daemon in embedded QuickJS. A package registers lifecycle hooks, tools, commands, providers, request policies, prompt sections, and client-neutral contributions through `@theway-ai/plugin-sdk`; it has no ambient filesystem, process, network, environment, credential, provider, persistence, daemon, or terminal access.

This document is the canonical reference for ABI v2 package structure, lifecycle hooks, actions, registrations, state, trust, reload, diagnostics, and the compaction compatibility format. The plugin development SDK is under `sdks/plugin`, with generated contract declarations and JSON Schemas under `sdks/plugin/abi-v2`.

## Package layout and discovery

Each ABI v2 extension is a directory containing a strict `theway-extension.json` manifest and its entry module. Project packages shadow global packages with the same extension ID.

```text
<cwd>/.theway/extensions/<extension-id>/theway-extension.json
<cwd>/.theway/extensions/<extension-id>/index.js
$THEWAY_DIR/extensions/<extension-id>/theway-extension.json
$THEWAY_DIR/extensions/<extension-id>/index.js
```

Discovery is deterministic. A malformed, unsupported, untrusted, or faulted package remains visible in the extension catalog with a structured diagnostic and does not prevent the daemon or unrelated packages from starting. The catalog orders effective packages by descending manifest priority, source layer, and extension ID.

### Manifest

```json
{
  "id": "workspace-policy",
  "version": "1.0.0",
  "abi": 2,
  "entry": "index.js",
  "priority": 100,
  "scope": "session",
  "stateSchema": 1,
  "permissions": ["session.write", "tools.register"],
  "optionalPermissions": ["tools.override"]
}
```

| Field | Required | Contract |
|---|---:|---|
| `id` | yes | 1–64 lowercase ASCII letters, digits, or single hyphens; it is the state, diagnostics, trust, and effect owner namespace. |
| `version` | yes | Semantic version string. |
| `abi` | yes | Must be `2`. |
| `entry` | yes | Non-empty package-relative module path without parent traversal. |
| `priority` | no | Signed ordering value; defaults to `0`, with larger values dispatched first. |
| `scope` | yes | `process`, `session`, `run`, or `request`; registrations cannot outlive the manifest scope. |
| `stateSchema` | no | Positive durable-state schema version; required when the package owns versioned state or migrations. |
| `permissions` | no | Required capabilities; denial blocks the package. |
| `optionalPermissions` | no | Capabilities the package can detect with `api.capabilities.has`; denial does not block loading. |

Unknown manifest fields, duplicate permissions, required/optional overlap, unsupported ABI values, and unsafe entry paths reject the package before entry evaluation.

## Trust and capabilities

Project packages require a recorded project or exact-package trust decision. Exact-package records bind the extension ID, canonical path, content digest, and requested permission set. Any content or permission expansion requires another decision. Global packages follow the configured global policy: allow declared permissions, require a record, or deny.

The TUI exposes the client-neutral protocol operations with these commands:

```text
/extensions
/extension-trust project <trusted|denied> [permissions...]
/extension-trust package <extension-id> <trusted|denied> [permissions...]
/extension-reload [--cancel]
/ext:<registered-command> {"argument":"value"}
```

Omitting permissions from a trusted TUI decision grants the selected catalog entries' declared permissions. A supplied list grants only that subset; every required permission must be present. Trust records are written atomically under the theway base directory and privileged decisions and broker calls produce redacted audit records.

| Permission | Public authority |
|---|---|
| `session.write` | Queue namespaced state mutations, custom events, persistent model context, and follow-up messages. |
| `tools.register` | Register a model-facing tool. |
| `tools.override` | Request an override of an existing tool; the displaced tool is restored when the effect ends. |
| `commands.register` | Register a daemon command. |
| `providers.register` | Register declarative provider/model metadata. |
| `client.contribute` | Publish validated client-neutral contributions. |
| `workspace.read` | Read a canonical path inside the allowed workspace roots. |
| `workspace.write` | Write a canonical path inside the allowed workspace roots. |
| `process.spawn` | Run a process through the daemon executor and cancellation policy. |
| `network.connect` | Make a brokered HTTP request subject to network policy and quota. |
| `provider.raw` | Register raw provider header/payload hooks and read invocation-local raw provider data. |
| `secrets.read:<name>` | Read exactly one named secret; wildcard secret access is invalid. |

Direct globals including `process`, `require`, `fetch`, `XMLHttpRequest`, `WebSocket`, environment access, and host resource objects are absent. Every broker checks the granted capability, cancellation state, invocation context, canonical path or target, and the per-invocation operation quota.

## TypeScript setup API

Install `@theway-ai/plugin-sdk` as a development dependency and leave its import external when compiling the extension. It is the only importable host module. `defineExtension` receives an async setup function once per extension instance. Setup returns registrations through handles; each handle supports idempotent `dispose()` and validated `update(descriptor)`.

```sh
npm install --save-dev @theway-ai/plugin-sdk
```

```ts
import { defineExtension } from "@theway-ai/plugin-sdk";

export default defineExtension(async (api) => {
  api.registerTool({
    name: "workspace-note",
    label: "Workspace note",
    description: "Write one note inside the workspace.",
    inputSchema: {
      type: "object",
      properties: { path: { type: "string" }, text: { type: "string" } },
      required: ["path", "text"],
    },
    scope: "session",
  }, async ({ arguments: args }) => {
    await api.workspace.writeText(args.path, args.text);
    return { content: [{ type: "text", text: `wrote ${args.path}` }], details: {} };
  });

  api.on("before_model_request", {
    priority: 20,
    payloadSchema: { type: "object", required: ["request"] },
  }, async ({ payload }) => ({
    abiMajor: 2,
    actions: [{
      kind: "replace_model_request",
      payload: { request: payload.request },
    }],
  }));
});
```

The setup API exposes `capabilities.has`, `workspace.readText/writeText`, `process.run`, `network.fetch`, `secrets.read`, `providerRaw.read`, `state.get/set/delete`, `events.replay/append`, `modelContext.append`, ephemeral `memory.get/set/delete/clear`, `migrateState`, all registration methods described below, and `on`. Broker methods are the public authority boundary; implementation globals and Rust types are not part of the ABI.

`api.on(event, descriptor?, handler)` validates `class`, `payloadSchema`, `allowedActions`, `priority`, `deadline`, `delivery`, and `failure`. A descriptor may narrow payload delivery and set priority, but any declared `allowedActions`, `deadline`, `delivery`, or `failure` must exactly match the canonical contract below.

## Hook execution contract

Every handler receives an `ExtensionEventEnvelope` containing `abiMajor`, `event`, `context`, and `payload`. Context always contains host-owned `extensionId`, `sessionId`, canonical `cwd`, and monotonic `sequence`; scoped run, turn, request, message, and tool-call IDs, the selected provider/model, interactive-client availability, cancellation state, and deadline appear when applicable. Extensions cannot replace host-owned context fields.

Hooks from effective packages run in catalog order. Registrations for the same event and class run by descending hook priority and then registration sequence. Transform hooks form a serial waterfall and the next hook sees the last valid value. Gate hooks terminate at the first `deny` or `cancel`. Observe hooks cannot mutate state through returned actions and their failure never changes the runtime operation.

| Class | Return contract | Allowed actions | Failure policy |
|---|---|---|---|
| `observe` | `null` or an empty batch | none | `continue`; failures are diagnosed and isolated. |
| `transform` | action batch without a decision | One event-specific replacement plus common secondary actions | `keep_last_value`; an invalid result preserves the last valid waterfall value. |
| `gate` | `abstain`, `allow`, `deny`, or `cancel` plus optional common secondary actions | Common secondary actions | `deny`; timeout, exception, disabled owner, or malformed output denies the operation. |
| `register` | action batch without a decision | `register_effect`, `dispose_effect`, `emit_diagnostic` | `reject_registration`; setup remains isolated from other packages. |

Common secondary actions are `set_state`, `delete_state`, `append_custom_event`, `append_model_context`, `enqueue_follow_up`, and `emit_diagnostic`. `input` additionally allows `emit_command_outcome`. Durable actions are fully validated and committed as one batch before ephemeral effects become visible; a persistence or quota failure rejects the entire batch.

The default host deadlines are 100 ms for `fast`, 500 ms for `standard`, and 2 s for `long`; deployments may configure the durations but not an event's deadline class. `message_update` and `tool_execution_update` observations use a bounded coalescing queue; all other delivery is inline.

### Canonical hook reference

The table lists the class selected when `descriptor.class` is omitted. Any event also accepts an explicit `observe` registration; `extension_load` additionally accepts `register`. “Common” means the common secondary actions defined above.

| Event | Default class | Payload | Primary/allowed actions | Deadline | Delivery | Failure |
|---|---|---|---|---|---|---|
| `extension_load` | observe | `{reason}` after setup and migration | none | long | inline | continue |
| `session_start` | observe | `{reason}` when the session instance starts | none | long | inline | continue |
| `input` | transform | `{message}` accepted from a client or follow-up | `replace_input`, `emit_command_outcome`, common | standard | inline | keep last value |
| `before_session_switch` | gate | branch `{kind,fromEntryId,toEntryId}` or session `{kind,targetSessionId}` | common | standard | inline | deny |
| `session_switched` | observe | completed branch/session switch and resulting IDs | none | standard | inline | continue |
| `before_session_fork` | gate | `{branchEntryId}` | common | standard | inline | deny |
| `session_forked` | observe | `{targetSessionId}` | none | standard | inline | continue |
| `before_model_selection` | gate | `{provider,model}` target | common | standard | inline | deny |
| `model_selected` | observe | `{provider,model}` selected | none | standard | inline | continue |
| `before_run` | transform | current run options | `patch_run_context`, common | standard | inline | keep last value |
| `run_started` | observe | run start metadata | none | standard | inline | continue |
| `turn_started` | observe | turn start metadata | none | standard | inline | continue |
| `context` | transform | `{messages}` before request assembly | `replace_context`, common | standard | inline | keep last value |
| `before_model_request` | transform | `{request}` normalized system/messages/tools/generation draft | `replace_model_request`, common | standard | inline | keep last value |
| `before_provider_request_headers` | transform | `{request}` typed provider header request | `replace_provider_headers`, common; requires `provider.raw` | standard | inline | keep last value |
| `before_provider_request_raw` | transform | `{request}` typed raw provider payload | `replace_provider_payload`, common; requires `provider.raw` | standard | inline | keep last value |
| `provider_response` | observe | `{response}` normalized provider response metadata | none | standard | inline | continue |
| `provider_request_failed` | observe | `{failure}` normalized provider failure | none | standard | inline | continue |
| `message_start` | observe | `{message}` initial message snapshot | none | standard | inline | continue |
| `message_update` | observe | `{message,updateKind}` streaming snapshot | none | fast | bounded coalescing | continue |
| `message_end` | transform | `{message}` accepted finalized message | `replace_message`, common | standard | inline | keep last value |
| `tool_call` | gate | `{assistantMessage,toolCall,args}` | common | standard | inline | deny |
| `tool_execution_start` | observe | `{toolName,args}` and scoped tool-call ID | none | standard | inline | continue |
| `tool_execution_update` | observe | `{toolName,update}` and scoped tool-call ID | none | fast | bounded coalescing | continue |
| `tool_execution_end` | observe | `{toolName,result,isError}` and scoped tool-call ID | none | standard | inline | continue |
| `tool_result` | transform | `{toolCall,args,result,isError}` | `replace_tool_result`, common | standard | inline | keep last value |
| `turn_completed` | observe | `{message,toolResults}` | none | standard | inline | continue |
| `run_ended` | observe | `{outcome}` terminal run result | none | standard | inline | continue |
| `run_error` | observe | `{category,message}` terminal error | none | standard | inline | continue |
| `run_settled` | observe | `{outcome}` after all run cleanup | none | standard | inline | continue |
| `before_compaction` | gate | `{algorithm,fromHook}` | common | standard | inline | deny |
| `compaction_succeeded` | observe | committed compaction metadata | none | standard | inline | continue |
| `compaction_failed` | observe | failure/cancellation metadata | none | standard | inline | continue |
| `session_shutdown` | observe | `{reason}` during session teardown | none | long | inline | continue |
| `extension_unload` | observe | `{reason}` before instance disposal | none | long | inline | continue |

### Lifecycle sequence

Package startup is `discovery → trust evaluation → isolated entry evaluation → setup registrations → state replay/migration → extension_load → session_start`. A normal run is `input → before_run → run_started → [turn_started → context → before_model_request → provider request/response → message lifecycle → tool lifecycle → turn_completed]* → run_ended or run_error → run_settled`. Session switching and forking place their `before_*` gate before the durable operation and their completed observation after rehydration. Shutdown is `session_shutdown → extension_unload → reverse-order effect disposal → QuickJS instance disposal`.

## Reversible registrations

Registrations are validated as a complete candidate set. Each accepted effect is owned by extension ID, session, scope, and conflict key. Ending a request, run, session, process, explicit handle, fault, reload, or shutdown disposes matching effects in reverse registration order. Disposal is idempotent.

| API | Descriptor contract | Required permission |
|---|---|---|
| `registerTool` | `name`, `label`, `description`, JSON `inputSchema`, optional `resultSchema`, `permission`, `scope`, and `override` | `tools.register`; override also requires `tools.override` |
| `registerCommand` | lowercase-hyphen `name`, `label`, `description`, object `argumentSchema`, optional `availability`, and `scope` | `commands.register` |
| `registerProvider` | `providerId`, HTTP(S) `baseUrl`, `format`, optional `credentialRef`, 1–256 `models`, and `scope` | `providers.register`; credential also requires `secrets.read:<credentialRef>` |
| `registerPromptSection` | `sectionId`, text, priority, provider/model/interactive predicate, and scope | none |
| `registerRequestPolicy` | `policyId`, priority, predicate, and scope plus a transform handler | none; actions still require their capabilities |
| `contribute` | validated contribution envelope with owner and scope | `client.contribute` |

Tool conflicts reject by default. An authorized `override: true` creates a restoration stack, so unloading or disposing the overriding registration restores the displaced tool. Command handlers return `success`, `rejected`, or `cancelled` outcomes and do not receive terminal APIs.

Provider `format` is exactly one of `openai_chat_completions`, `openai_responses`, or `anthropic_messages`. Each model declares `id`, `name`, optional `reasoning`, input modalities, positive `contextWindow`, and positive `maxTokens` not exceeding the context window. The existing provider serializers own the wire protocol; extensions register metadata rather than executable transport code.

Availability and request predicates contain optional exact `providers` and `models` sets plus `requiresInteractiveClient`. Empty sets match all values. Prompt sections are appended by priority before request policies; policies execute as normal `before_model_request` transforms.

Client contributions use one of these declarative payloads:

| Kind | Fields |
|---|---|
| `notification` | `level`, `title`, `body` |
| `status_item` | `label`, `value`, optional `detail` |
| `command` | validated command descriptor |
| `detail_panel` | `title`, object/array `data` |
| `form_action` | `title`, object `schema`, `submitCommand` |

Clients render supported kinds from transport data only and ignore unknown kinds. Contributions and routine lifecycle status never append synthetic assistant messages or operation-log lines to the conversation feed.

## Durable state, context, and migration

`api.state` stores extension-private branch-local values by key. `api.events.append` records namespaced custom events and `api.events.replay` returns them in branch order. `api.modelContext.append` stores stable `contextId` entries placed as a system-prompt section or a model message; identical IDs replace their prior projection, so restored context is exact-once. `api.memory` is session-instance-local scratch state and is never serialized.

Durable writes are queued during one hook. The host validates action kind, owner, ABI, state schema, origin sequence, IDs, content placement, action count, entry size, and per-extension quota before one append. Persistence failure leaves durable projection and ephemeral effects unchanged.

On resume, branch switch, fork, or runtime relocation, the host reconstructs state and model context only from entries on the selected branch. A package whose manifest raises `stateSchema` registers one `api.migrateState(handler)` callback. Migration runs before normal session hooks, appends the migrated values and a migration marker on success, and faults only that package while retaining historical entries on failure.

## Reload and diagnostics

Catalog change detection and explicit reload validate every candidate in isolated QuickJS instances before replacing the active catalog. If a run or tool execution is active, reload remains pending until `run_settled` or the tool settlement boundary; `--cancel` requests controlled cancellation but does not swap an instance before settlement. Application order is old cleanup hooks, broker cancellation, reverse effect disposal, candidate load, state replay/migration, lifecycle startup, catalog publication, and revision increment. A failed candidate leaves the active catalog and its registrations intact.

Catalog status and diagnostics are exposed over gRPC, HTTP JSON-RPC, SSE/WebSocket snapshots, the typed client, and the TUI. Diagnostics contain host-owned extension/session/event/sequence metadata, stable code and severity, public details, and names of redacted fields. Secret values, raw credentials, private extension state, model content, and privileged broker arguments are not diagnostic fields. `emit_diagnostic` accepts only the public diagnostic payload and the host supplies ownership and correlation metadata after the action batch commits.

Three consecutive invocation failures open the session-local circuit breaker. Opening a circuit disables the affected owner, cancels its brokers, and reverses its effects; required gate semantics remain fail-closed. A successful invocation resets the consecutive-failure count.

## DeepSeek Anchor reference package

The reference package lives under `extensions/deepseek-anchor`. Copy the directory into a project or global extension root, review `anchor-config.json`, record trust, and reload the catalog. The shipped configuration uses `zeroAnchor: true`, so installation is inert until explicitly enabled.

| Configuration | Meaning |
|---|---|
| `providerPredicates` | Non-empty glob list matched against the normalized provider ID. |
| `modelPredicates` | Non-empty glob list matched against the normalized model ID. |
| `bootstrapPrompt` | Minimal system instruction used only for a matching unpromoted request unless persona scope is session-wide. |
| `promotionCondition` | `first_assistant`, `first_tool_call`, or `assistant_or_tool_call`, with optional text regex and tool-name filter. |
| `personaScope` | `bootstrap_only` removes the bootstrap persona after promotion; `session` prepends it to later matching requests. |
| `bootstrapTokenLimit` | Optional positive override used only during bootstrap; omission preserves the model/request default. |
| `restoredContext` | Stable exact-once system-prompt context appended at promotion. |
| `maxEditorOutputChars` | Positive output clipping limit for editor views. |
| `zeroAnchor` | When true, bypass all transformations and tool registration without deleting branch-local promotion state. |

When enabled and matched, an unpromoted request receives only `bash` and a compatible `str_replace_editor`, the configured bootstrap prompt, an empty message list, and the unchanged token limit unless `bootstrapTokenLimit` is present. Missing or incompatible required schemas reject the complete transform, so the base request remains intact. The editor implements `view`, `create`, unique-literal `str_replace`, and line `insert` exclusively through `api.workspace`.

An accepted finalized assistant message matching the promotion condition atomically stores the idempotent promotion marker, a custom promotion event, and the restored model context before the next request. Promoted requests retain the immutable full base tool catalog and retained conversation. Resume and branch switch reconstruct the phase from the selected branch; forks before promotion remain bootstrap, forks after promotion remain promoted, concurrent sessions are isolated, and temporary non-matching model selection bypasses Anchor without erasing phase state.

The same normalized bootstrap and promotion behavior is exercised through OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages provider serializers. DeepSeek- and Anchor-specific policy remains entirely in this package; core and provider crates expose only generic lifecycle and normalized request seams.

## Compaction compatibility format

Top-level `.ts` files directly under either extension root may declare `export const kind = "compaction"`. This compatibility format supports `decide_compact`, `select_cut_point`, and `summarize_prefix`, with missing, null, invalid, or failed hook results falling back to the built-in compaction algorithm.

Compatibility files run each hook in a fresh restricted QuickJS context. They receive no ABI v2 setup object, brokers, permissions, state, registrations, lifecycle hooks, or client contributions. Package directories with `theway-extension.json` are the format for all general runtime extensions.

## Verification

```bash
cargo test -p theway-contract --test extension
cargo test -p theway-daemon --test deepseek_anchor_extension
cargo test -p theway-daemon --test ts_extension_dispatcher
cargo test -p theway-daemon --test ts_extension_state
make fmt-check
make file-size-check
make layering-check
make doc-sync
make lint
make test
```
