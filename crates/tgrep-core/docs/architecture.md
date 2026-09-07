# tgrep-core architecture

English | [中文](architecture.zh.md)

Vendored upstream: [microsoft/tgrep](https://github.com/microsoft/tgrep) at
commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`. MIT.

## What this crate is

The trigram index library behind the `tgrep` CLI — a full-text index over a
repository's text files that lets regex queries touch only candidate files
instead of walking the tree:

- `builder` — walks the tree (gitignore-aware), extracts trigrams, and writes
  the on-disk index (`<root>/.tgrep`); external merge strategy bounds peak
  memory on huge repos.
- `reader` / `ondisk` — mmap'd index reader with binary-searched trigram
  lookup tables.
- `live` / `hybrid` — in-memory overlay (files changed after server start)
  merged over the disk index; overlay takes precedence.
- `query` — trigram query planning (sorted posting-list intersection/union)
  and candidate-file regex search.
- `walker` — file walking with binary rejection, size caps, and gitignore /
  `core.ignorecase` handling.
- `path_index` — filename index for `--files` unions.

## Boundaries with theway

- Theway does **not** link this crate. The daemon shells out to the `tgrep`
  binary built from `crates/tgrep-cli`; server lifecycle (spawn/readiness/LRU/
  reaping) is implemented in the daemon (`crates/theway-daemon/src/tgrep_server.rs`,
  issue #121).
- The vendored `Cargo.toml` is self-contained (independent metadata); sources
  under `src/` are byte-identical to upstream and must stay that way.
