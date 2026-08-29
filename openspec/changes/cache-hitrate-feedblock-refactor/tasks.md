# Tasks: cache-hitrate-feedblock-refactor

Issue: #52 (cache hit rate) + #51 (FeedBlock protocol). Breaking wire cleanup; no compatibility aliases.

graph TD
  1-wire-usage --> 2-cache-tracker
  2-cache-tracker --> 3-daemon-metrics
  3-daemon-metrics --> 4-tui-stats
  1-wire-usage --> 5-feedblock
  5-feedblock --> 6-verify

## 1. Wire / Usage Refactor

- [x] 1.1 Update `WireContextUsage`: replace `input_tokens` / `total_tokens` with `cached_tokens`, `new_tokens`, `total_input_tokens`, `cache_write_tokens`, `provider_cache_hit_rate`, `prefix_cache_hit_rate`
- [x] 1.2 Update proto `session.proto` and transport wire codecs (`resources.rs`, `wire.rs`, `proto.rs`)
- [x] 1.3 Regenerate client SDK (`sdks/client`) and update all in-repo test fixtures
- [x] 1.4 Remove deprecated fields from all constructors/tests; do not keep aliases

## 2. Cache Hit Rate Tracker (core)

- [x] 2.1 Add `context_cache.rs` with canonical context serialization and chunked hashing
- [x] 2.2 Implement longest-common-prefix overlap and byte-to-token calibration
- [x] 2.3 Add `ContextCacheTracker` keyed by `(session_id, provider, model)` with reset on model/provider switch
- [x] 2.4 Hook tracker into `call_llm`: compute prefix hit before request, record provider usage after stream
- [x] 2.5 Unit tests: append-only high hit, mid-insert miss, compaction miss, virtualization stability, provider missing cache fields

## 3. Daemon Metrics Aggregation

- [x] 3.1 Accumulate `cached_tokens`, `new_tokens`, `total_input_tokens`, `cache_write_tokens`, `prefix_hit_tokens` into session cumulative usage
- [x] 3.2 Expose per-turn and session-cumulative `provider_cache_hit_rate` / `prefix_cache_hit_rate` in snapshots
- [x] 3.3 Update daemon tests for usage aggregation and snapshot output

## 4. TUI Dual Metrics

- [x] 4.1 Replace old single hit percentage with `cache X% · prefix Y%`
- [x] 4.2 Update `stats.rs` helpers and tests
- [x] 4.3 Update any feed/status rendering that consumes removed wire fields

## 5. FeedBlock Protocol Refactor

- [x] 5.1 Add `tool_call` FeedBlock variant (name, args, metadata)
- [x] 5.2 Add `error` FeedBlock variant (message, code, recoverable)
- [x] 5.3 Remove `tool` variant and error-prefixed `plain` emission
- [x] 5.4 Update all feed producers: session feed, `GetHistory`, graph node streaming
- [x] 5.5 Update proto/wire/SDK and all FeedBlock consumers/tests

## 6. Verification & Close

- [ ] 6.1 `cargo test -p theway-transport -p theway-core -p theway-daemon -p theway-tui` green
- [x] 6.2 `cargo clippy --workspace --all-targets -- -D warnings` green (per-crate if probe build blocks workspace)
- [x] 6.3 `cargo fmt --all --check` green
- [x] 6.4 SDK regeneration check (`make sdks-check` or repo sdk sync)
- [x] 6.5 Push commits and close #51, #52
