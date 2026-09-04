# theway-extensions

Leaf data crate: official runtime extension packages embedded as `include_str!` constants (`packages/<id>/`), plus `ensure_managed_installed(base)` for atomic, idempotent provisioning into the managed extensions layer. No runtime dependencies; depended on by theway-daemon only.

Layout: `Cargo.toml`, `src/lib.rs` (constants + provisioning + tests), `packages/{tui-docs,deepseek-anchor}/` (package sources, single source of truth).

Conventions:
- Keep package sources under `packages/<id>/`; embed with `include_str!`, never duplicate content.
- `SHIPPED_PACKAGES` is the managed-layer allowlist; reference packages join `ALL_PACKAGES` only.
- Provisioning must stay best-effort (warn, never fail startup) and directory-atomic (staging + rename).
- When a package file changes, the embedded content changes with the next build — no regeneration step exists or is needed.

Validation: `cargo test -p theway-extensions`, `cargo doc -p theway-extensions --no-deps --document-private-items`, `make layering-check`; daemon-side discovery is covered by theway-daemon's extension test suites.
