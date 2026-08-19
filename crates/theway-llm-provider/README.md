# theway-llm-provider (Rust)

Rust port of [`@earendil-works/theway-llm-provider`](https://github.com/earendil-works/pi) — a unified streaming
LLM client. Every supported model (Anthropic, OpenAI Responses / Completions / Codex, Google
Gemini, Vertex, Bedrock, Mistral, Azure, Cloudflare, OpenAI-compatible endpoints) is reached
through a single `stream(model, context, options)` call that yields a normalized
`AssistantMessageEventStream`.

## Status

A 1:1 source-level port mirroring the TypeScript file layout. All ten wire-protocol providers are
implemented (happy-path streaming, message conversion, tool calls, thinking/reasoning, caching),
plus the cross-cutting utilities. Remaining TODOs are noted per-file (advanced auth transports,
adaptive thinking variants, image generation).

| Layer | Status |
|-------|--------|
| Core types (`types.rs`) | implemented |
| Event stream (`utils/event_stream.rs`) | implemented + tested |
| Registry + entry points (`api_registry.rs` / `stream.rs`) | implemented |
| Anthropic | working — SSE, text/thinking/tool_use, cache_control, budget thinking, compat |
| OpenAI Responses | working — text/reasoning/function_call, cache key + 24h retention |
| OpenAI Completions | working — catch-all; parallel tool_calls, reasoning_content, `[DONE]` |
| OpenAI Codex | working (HTTP/SSE path) — instructions body, codex headers; WebSocket transport TODO |
| Azure OpenAI Responses | working — reuses Responses consumer; deployment-name + api-key header |
| Google Gemini | working — text/thought parts, functionCall, thinking budget |
| Google Vertex | working (Bearer token) — reuses Gemini consumer; full ADC/JWT TODO |
| Amazon Bedrock | working (Bearer token) — Converse Stream + AWS eventstream decoder; SigV4 TODO |
| Mistral | working — chat-completions-shaped; x-affinity, alphanumeric tool ids |
| Cloudflare | base-url placeholder resolver (rides on openai-completions) |
| Faux | scriptable — queue `AssistantMessage`s, replayed as event sequences |
| Models registry | 938 models loaded from `models.generated.json` |
| Cross-provider transform_messages | implemented (image downgrade, thinking, id normalize, synthetic results) |
| Context-overflow detection (`overflow.rs`) | implemented — all provider error patterns |
| Anthropic OAuth (PKCE) | implemented — authorize URL, local listener, exchange, refresh |
| OpenAI Codex / Copilot OAuth, Images | stub |

Mock-SSE end-to-end tests (`tests/anthropic_sse_e2e.rs`) prove the shared HTTP→SSE→event pipeline
against a local server (no API key). `scripts/regen_models.sh` regenerates the model catalog from
the TS source of truth.

### Tests

```
cargo test --features all-providers     # 55 unit + 3 SSE e2e + 4 catalog
```

## Layout

```
src/
  lib.rs                         barrel
  types.rs                       core types
  stream.rs                      stream() / streamSimple() / complete()
  api_registry.rs                registerApiProvider / getApiProvider
  models.rs / models_generated.rs
  images.rs / image_models.rs
  env_api_keys.rs
  oauth.rs
  session_resources.rs
  providers/
    anthropic.rs
    openai_responses.rs
    openai_completions.rs
    ...                          one file per wire protocol or vendor
  utils/
    event_stream.rs
    sanitize_unicode.rs
    json_parse.rs
    ...
    oauth/
      anthropic.rs
      github_copilot.rs
      ...
```

The TypeScript source is at `/Users/dongxu/pi-rs/packages/ai/`. Each Rust file has the same name
as its TS counterpart (snake_case instead of kebab-case).

## Build

```bash
cargo check --no-default-features          # registry/types, no concrete provider
cargo check --no-default-features --features amazon-bedrock
cargo build --features all-providers
cargo run --example anthropic_hello --features anthropic   # needs ANTHROPIC_API_KEY
```

Provider features compile only their implementation modules and provider-specific dependencies. In particular, `amazon-bedrock` owns the SigV4/HMAC stack and `faux` owns UUID generation; neither dependency tree is present in a no-default-features build.
