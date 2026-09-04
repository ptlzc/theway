# theway-extensions architecture

English | [中文](architecture.zh.md)

## Ownership and dependency direction

`theway-extensions` is a leaf data crate: it depends on nothing at runtime and embeds the official extension package sources as constants. The daemon depends on it; it depends on no other workspace crate, so it sits near the leaf end of the layering order (before `theway-daemon` in the publish allowlist).

## Data model

- `EmbeddedPackage { id, files }` — a manifest id plus the package directory as `(relative_path, content)` pairs.
- `TUI_DOCS` / `DEEPSEEK_ANCHOR` — the two official packages. Package files are embedded with `include_str!("../packages/<id>/…")`; the on-disk `packages/` directory stays the single source of truth, so a rebuild picks up edits.
- `SHIPPED_PACKAGES` — provisioned into the managed layer; `ALL_PACKAGES` — shipped + reference, for tests and tooling.

## Managed-layer provisioning

`ensure_managed_installed(base)` writes `SHIPPED_PACKAGES` into `<base>/extensions-managed/<id>/`:

1. Compare each embedded file against the installed copy; skip the package when everything matches.
2. Stage the whole package into `extensions-managed/.<id>-staging`, then rename over the target — atomic at the directory level, so the catalog never observes a half-written package.
3. Failures only warn (`tracing::warn`); a missing managed copy degrades to "pointer package absent", never a startup failure.

The daemon calls this from `SessionExtensionResources::new` right before `ExtensionRegistry::discover`, so the catalog's managed layer sees the embedded packages on every startup. Managed packages are granted their declared permissions without a trust record (platform-shipped, user read-only), and project/user packages with the same id still shadow them.

## Validation

Unit tests assert manifest validity (parseable JSON, `id` matches the constant name, entry file embedded), materialization of missing packages, idempotence, stale-content refresh, whole-directory replacement, and staging cleanup.
