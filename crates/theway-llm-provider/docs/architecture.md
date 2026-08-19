# theway-llm-provider architecture

English | [中文](architecture.zh.md)

## Normalized data model

[`types.rs`](../src/types.rs) defines the provider-independent model:

- `Model` identifies a provider, wire API, endpoint, capabilities, context window, cost, and supported thinking levels.
- `Context` contains ordered user, assistant, and tool-result messages plus tool definitions.
- `StreamOptions` contains request-level limits, thinking, cache, transport, headers, abort, and key-resolution behavior.
- `AssistantMessageEvent` is the ordered streaming protocol for text, thinking, tool-call deltas, completion, and error.
- `AssistantMessage` is the collected terminal result with content, usage, stop reason, provider/model identity, and optional error.

Provider modules convert only between this model and an external wire protocol. Runtime concepts such as `AgentMessage`, tool scheduling, session entries, or permission decisions do not cross this crate.

## Provider registration and dispatch

[`api_registry.rs`](../src/api_registry.rs) defines `ApiProvider` and a process registry keyed by API id. A lookup returns an owned `RegisteredHandle`, so a request already resolved to a provider can finish even if its registration source is removed concurrently.

[`providers/register_builtins.rs`](../src/providers/register_builtins.rs) registers enabled built-ins once through `OnceLock`. Cargo features control which implementations and provider-specific dependencies compile. Runtime extensions may register providers with a source id and later unregister every provider from that source.

[`stream.rs`](../src/stream.rs) ensures built-ins are registered, resolves `model.api`, and delegates to the provider. A missing or mismatched provider produces the same terminal error-stream form as a provider failure. `complete` consumes the stream's result instead of implementing a second request path.

## Request and stream pipeline

Before a provider sends a request, [`providers/transform_messages.rs`](../src/providers/transform_messages.rs) reconciles history with target-model capabilities. It can downgrade unsupported images, convert incompatible thinking blocks, normalize tool-call ids, remove unusable error turns, and synthesize missing tool results so provider APIs receive a valid tool sequence.

Each provider module under [`providers/mod.rs`](../src/providers/mod.rs) owns request-body construction, authentication headers, endpoint selection, stream decoding, usage mapping, stop-reason mapping, and protocol-specific tool/thinking conversion. Shared Responses, Google, prompt-cache, SSE, AWS event-stream, retry, overflow, validation, and Unicode helpers stay in their corresponding shared modules under `providers/` or [`utils/mod.rs`](../src/utils/mod.rs).

[`utils/event_stream.rs`](../src/utils/event_stream.rs) couples `AssistantMessageEventSender` with `AssistantMessageEventStream`. The sender publishes ordered events and resolves one final `AssistantMessage`; the stream supports both incremental consumption and awaiting the terminal result.

Cancellation and provider failures terminate the stream with normalized stop/error records. Provider modules must not leave callers waiting for a terminal result after their network task exits.

## Catalogs and credentials

[`models.rs`](../src/models.rs) overlays runtime custom models on the generated catalog in [`models_generated.rs`](../src/models_generated.rs) and [`models.generated.json`](../src/models.generated.json). [`image_models.rs`](../src/image_models.rs) provides the corresponding image catalog.

[`env_api_keys.rs`](../src/env_api_keys.rs) maps provider identifiers to environment keys. Callers may also supply `get_api_key` through stream options; the provider pipeline consumes resolved credentials but does not persist them.

[`session_resources.rs`](../src/session_resources.rs) exposes cleanup for provider-owned session resources. Long-lived connection caches remain provider infrastructure and must not become agent-session state.

## Adding a provider

1. Add a feature and only the dependencies required by that protocol in [`Cargo.toml`](../Cargo.toml).
2. Implement `ApiProvider` in a provider module, reusing shared request/stream helpers where the wire protocol is genuinely shared.
3. Register the implementation under the same feature in `providers/register_builtins.rs`.
4. Map every content, tool, thinking, usage, stop, cancellation, and error path to normalized types.
5. Add keyless request/stream fixtures and feature-matrix compilation coverage.

## Invariants

- Provider-specific JSON, headers, event names, and error bodies remain inside provider modules.
- Every started stream reaches exactly one normalized terminal result.
- Tool-call deltas preserve provider correlation while producing stable normalized ids and argument text.
- Message transformation preserves valid history ordering and never sends unsupported content silently.
- Credentials remain caller- or environment-supplied and are not stored in catalogs, messages, diagnostics, or session resources.
- Feature-disabled providers and their optional dependency trees do not compile into thin builds.
