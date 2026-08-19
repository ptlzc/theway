# Pager rendering modification rules

This file contains the complete crate-local modification rules for `theway-pager-render`. Read the module contract in [`docs/architecture.md`](docs/architecture.md) before changing rendering behavior.

## Ownership

- Keep session, protocol, event-loop, key-binding, and target-opening policy in the caller.
- Keep line operations style-preserving, grapheme-aware, and based on terminal display width.
- Require explicit geometry and path context where behavior depends on viewport or working-directory state.
- Keep URL annotation limited to safe supported schemes; annotation must not execute or open the target.

## Compatibility

- Keep code-lineage details in [`NOTICE`](NOTICE).
- Preserve resolved targets independently of abbreviated display labels.
- Add focused tests next to the affected module; place a multi-file suite under `tests/<mirrored-src-path>/` and bridge it from the owning source module.

## Verification

Run `cargo test -p theway-pager-render` and `cargo doc -p theway-pager-render --no-deps --document-private-items`.
