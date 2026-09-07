# tgrep-cli (vendored)

English | [中文](README.zh.md)

Vendored copy of [`tgrep-cli`](https://github.com/microsoft/tgrep) — the `tgrep`
binary (trigram-indexed grep with client/server architecture). Copied **verbatim**
from upstream commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab` (2026-09-07).

- License: MIT (see `LICENSE`)
- Upstream: <https://github.com/microsoft/tgrep>
- Why vendored: not published on crates.io; theway builds it as a workspace member
  and the daemon spawns `tgrep serve` + `tgrep` client queries for the built-in
  grep tool (issue #121). The binary installs alongside `theway`/`thewayd`.
- Upgrade process: download the upstream tarball at the new pinned commit and
  replace `src/` verbatim. `Cargo.toml` is NOT verbatim: theway's workspace
  lacks `[workspace.dependencies]`/inheritance-compatible metadata, so the
  manifest is self-contained with upstream's exact version/deps — when
  upgrading, re-apply only the dependency/version values upstream changed.
  Do **not** hand-edit vendored sources — report bugs upstream instead.
