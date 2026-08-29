## Context

Current `WireContextUsage` exposes `input_tokens` whose semantics are ambiguous: some providers report total input including cache reads while the internal `Usage.total_tokens` treats `input` as non-cached new input. The TUI derives `cache_read / input_tokens` without a clear denominator. Separately, FeedBlock uses generic `tool` and error-prefixed `plain`, forcing consumers to parse string prefixes.

Theway already has provider cache reporting (`cache_read_tokens`, `cache_write_tokens`), deterministic tool-result virtualization, compaction, and provider-specific prompt cache controls. These features make prefix stability the main lever for KV cache hit rate.

## Goals / Non-Goals

**Goals:**
- Make cache usage wire fields unambiguous and remove deprecated fields.
- Provide per-session provider hit rate and prefix hit rate.
- Add a provider-agnostic prefix-overlap estimator that works without a tokenizer.
- Add first-class FeedBlock `tool_call` / `error` variants and remove `tool` / error-prefixed `plain`.
- Keep all consumers (daemon, TUI, SDK, tests) updated in the same breaking change.

**Non-Goals:**
- Real tokenizer integration or embedding-based token counting.
- Cross-session shared prefix caching (fork inheritance can be added later).
- Provider cache-control placement changes (existing `cache_control` / `prompt_cache_key` behavior stays).

## Decisions

### 1. Breaking wire: explicit cache usage fields

Replace `WireContextUsage.input_tokens` with:

```proto
uint64 cached_tokens = 1;
uint64 new_tokens = 2;
uint64 total_input_tokens = 3;
uint64 output_tokens = 4;
uint64 cache_write_tokens = 5;
double provider_cache_hit_rate = 6;
double prefix_cache_hit_rate = 7;
```

- `new_tokens` is the non-cached input reported by providers (the old internal `input` field).
- `total_input_tokens = cached_tokens + new_tokens`.
- Remove `input_tokens` and `total_tokens` from the wire shape; internal `Usage` may keep them during migration but the wire is the source of truth.

**Alternative considered:** keeping `input_tokens` as an alias. Rejected because the user explicitly wants no tech debt and no compatibility shims.

### 2. Client-side prefix overlap: chunked hash + LCP

Add `ContextCacheTracker` in `theway-core`:

- Canonicalize the final provider `Context` (system prompt, messages, tools) into a stable byte sequence at the same point where the provider body is about to be built.
- Split into fixed-size chunks (256 bytes) and hash each chunk.
- Compare with the previous request's chunk list from index 0; count contiguous equal chunks.
- Convert byte overlap to token estimate:
  ```
  tokens_per_byte = total_input_tokens / total_bytes
  prefix_hit_tokens = overlap_bytes × tokens_per_byte
  ```
- Store the current chunk list as the next baseline, keyed by `(session_id, provider, model)`.

**Alternative considered:** rolling hash for longest common substring. Rejected because KV caches are prefix caches; LCP is sufficient and simpler.

**Alternative considered:** real tokenizer. Rejected because it adds a heavy dependency and the estimate only needs to explain trends.

### 3. Hook placement

In `call_llm`:

- After `transform_context` / `convert_to_llm` / `transform_model_request`, compute the canonical context and prefix hit estimate.
- After the stream completes, read `Usage` and compute provider hit rate.
- Pass both into the per-session metrics accumulator.

The tracker is injected via `AgentOptions` / `AgentHarness` so daemon and TUI can read session metrics.

### 4. FeedBlock first-class variants

Update the wire/proto FeedBlock:

- Add `tool_call` variant with `name`, `args`, and tool-call metadata.
- Add `error` variant with `message`, `code`, `recoverable`.
- Remove `tool` variant and stop emitting `plain` with `error:` prefix.
- Keep `plain` only for non-error plain text.

All feed producers (session feed, `GetHistory`, graph node streaming) use the same variants.

### 5. Per-session metrics propagation

- Daemon accumulates `cached_tokens`, `new_tokens`, `total_input_tokens`, `cache_write_tokens`, `prefix_hit_tokens` into `session.cumulative_usage`.
- Snapshot exposes both per-turn and session-cumulative rates.
- TUI replaces the old single hit percentage with dual metrics:
  ```
  cache 72.3% · prefix 88.1%
  ```

## Risks / Trade-offs

- [Byte-to-token calibration is approximate] → Use provider-reported `total_input_tokens` for calibration; document that prefix hit rate is an estimate.
- [Provider `cache_read_tokens` semantics vary] → Normalize to `total_input_tokens = cached + new`; when a provider does not report cache reads, mark provider hit rate as unknown.
- [Breaking wire changes affect SDK/consumers] → Update all in-repo consumers and regenerate SDK in the same change; no aliases.
- [FeedBlock removal breaks external clients] → This is intentional per the issue; update WorkMate-facing docs and wire tests.

## Migration Plan

1. Change proto/wire types and regenerate SDK.
2. Update daemon snapshot and session usage aggregation.
3. Add `ContextCacheTracker` and hook it into `call_llm`.
4. Update TUI stats display.
5. Update FeedBlock producers and consumers.
6. Run full per-crate tests/clippy/fmt and commit as one breaking change.
