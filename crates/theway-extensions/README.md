# theway-extensions

English | [中文](README.zh.md)

`theway-extensions` bundles the official theway runtime extension packages as build-time data: package sources under `packages/<extension-id>/` are embedded with `include_str!` and exposed as constants, so the daemon can provision them into the managed extensions layer (`<base>/extensions-managed/`) at startup. The crate has no runtime dependencies.

The plugin ABI is unversioned (see `docs/extensions.md`), so extension packages must ship with a matching daemon; embedding them in this crate couples the two through the workspace version.

| Constant | Package | Role |
|---|---|---|
| `TUI_DOCS` | `tui-docs` | Prompt-section pointer to the bundled theway configuration guide; in `SHIPPED_PACKAGES`. |
| `DEEPSEEK_ANCHOR` | `deepseek-anchor` | Reference package for the extension docs; inert by default (`zeroAnchor: true`), not shipped. |

`ensure_managed_installed(base)` materializes `SHIPPED_PACKAGES` into `<base>/extensions-managed/` idempotently: each package directory is staged and renamed atomically, refreshed only when content differs. The daemon calls it before extension discovery.

## Documentation

- [Architecture](docs/architecture.md)

## Validation

```bash
cargo test -p theway-extensions
cargo doc -p theway-extensions --no-deps --document-private-items
make layering-check
```
