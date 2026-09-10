# tgrep-core (vendored)

English | [中文](README.zh.md)

Vendored copy of [`tgrep-core`](https://github.com/microsoft/tgrep) — the trigram
index library behind the `tgrep` CLI. Copied **verbatim** from upstream commit
`e2007b52d2b8fe4176159d0da20c9ba4a46d5aab` (2026-09-07).

- License: MIT (see `LICENSE`)
- Upstream: <https://github.com/microsoft/tgrep>
- Why vendored: not published on crates.io; theway keeps it under `crates/`
  beside `tgrep-cli` so the `tgrep` binary builds from the same pinned source.
- Workspace boundary: the root `Cargo.toml` lists this crate in
  `[workspace.exclude]`, so it is not a root workspace member and declares its
  own `[workspace]`. `tgrep-cli` consumes it through the path dependency
  `tgrep-core = { path = "../tgrep-core" }`; run this crate's checks with
  `cargo test --manifest-path crates/tgrep-core/Cargo.toml` or
  `make vendored-test`.
- Upgrade process: download the upstream tarball at the new pinned commit and
  replace `src/` verbatim. `Cargo.toml` is NOT verbatim: theway's workspace
  lacks `[workspace.dependencies]`/inheritance-compatible metadata, so the
  manifest is self-contained with upstream's exact version/deps — when
  upgrading, re-apply only the dependency/version values upstream changed.
  Do **not** hand-edit vendored sources — report bugs upstream instead.
