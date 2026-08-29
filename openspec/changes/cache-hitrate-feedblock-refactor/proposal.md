## Why

Session cache-hit reporting is ambiguous (`input_tokens` mixes cached and non-cached tokens) and there is no per-session prefix-overlap estimate, while FeedBlock's generic `tool` and error-prefixed `plain` blocks force fragile client-side parsing. Both need a breaking wire cleanup so the protocol is self-describing and cache behavior is measurable.

## What Changes

- **BREAKING**: Replace ambiguous `WireContextUsage.input_tokens` semantics with explicit `cached_tokens` / `new_tokens` / `total_input_tokens`; remove deprecated/ambiguous fields instead of keeping aliases.
- Add per-session cache hit rate metrics:
  - `provider_cache_hit_rate = cache_read_tokens / total_input_tokens`
  - `prefix_cache_hit_rate = prefix_hit_tokens / total_input_tokens`
- Add a client-side prefix-overlap estimator over the final provider context (after transforms), using chunked hashing + longest-common-prefix; no tokenizer or embedding dependency.
- **BREAKING**: Add first-class FeedBlock variants `tool_call` and `error`; remove `tool` and error-prefixed `plain` convention.
- Update daemon snapshot, TUI status display, SDK/generated wire types, and tests.

## Capabilities

### New Capabilities
- `telemetry/cache-hit-rate`: per-session provider and prefix cache hit rate metrics, wire fields, and client-side prefix overlap algorithm.
- `feed-protocol`: first-class FeedBlock `tool_call` / `error` variants and removal of ambiguous `tool` / error-prefixed `plain`.

### Modified Capabilities
- `snapshot-wire-diet-resync`: session snapshot wire shape changes for cache usage fields and FeedBlock variants.

## Impact

- `crates/theway-transport`: proto, wire types, codecs, generated SDK.
- `crates/theway-daemon`: snapshot/usage aggregation, session cumulative usage, FeedBlock production.
- `crates/theway-core`: context cache tracker, prefix overlap algorithm, `call_llm` hook.
- `crates/theway-tui`: stats display, feed rendering.
- `sdks/client`: regenerated protocol types.
- Consumers of gRPC FeedBlock and `WireContextUsage` (breaking changes).
