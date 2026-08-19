# Pager rendering modification rules

This file applies to `crates/theway-pager-render/`. Follow the workspace rules in [`../../AGENTS.md`](../../AGENTS.md) and the module contract in [`docs/architecture.md`](docs/architecture.md).

## Ownership

- Keep session, protocol, event-loop, key-binding, and target-opening policy in the caller.
- Keep line operations style-preserving, grapheme-aware, and based on terminal display width.
- Require explicit geometry and path context where behavior depends on viewport or working-directory state.
- Keep URL annotation limited to safe supported schemes; annotation must not execute or open the target.

## Compatibility

- Keep code-lineage details in [`NOTICE`](NOTICE).
- Preserve resolved targets independently of abbreviated display labels.
- Add focused tests next to the affected module unless a multi-file suite is required by [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md).

## Verification

Run `cargo test -p theway-pager-render` and `cargo doc -p theway-pager-render --no-deps --document-private-items`.
