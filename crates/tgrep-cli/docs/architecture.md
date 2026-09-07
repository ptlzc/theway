# tgrep-cli architecture

English | [中文](architecture.zh.md)

Vendored upstream: [microsoft/tgrep](https://github.com/microsoft/tgrep) at
commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`. MIT.

## What this crate is

The `tgrep` binary — trigram-indexed grep with a client/server architecture:

```
tgrep <pattern> <path> ---TCP JSON-RPC---> tgrep serve <root> (multi-client)
   (client, per query)                      |  HybridIndex (disk mmap + live overlay)
                                            |  notify file watcher, background indexer,
                                            |  periodic flush, hourly reconcile
```

- Client search fallback chain: running server (`<root>/.tgrep/serve.json`)
  → on-disk index → brute-force walk. Every path returns complete results;
  only speed differs.
- `--json` emits a ripgrep-compatible NDJSON stream (`begin`/`match`/`context`/
  `end`/`summary` records) — the integration contract theway's grep tool
  consumes.

## Boundaries with theway

- The daemon spawns `tgrep serve <root>` and runs `tgrep ... --json` client
  queries for the built-in grep tool. Spawn/readiness/LRU/reaping lives in the
  daemon (`crates/theway-daemon/src/tgrep_server.rs`, issue #121); nothing in
  this crate knows about theway.
- The vendored `Cargo.toml` is self-contained (independent metadata); sources
  under `src/` are byte-identical to upstream and must stay that way.
