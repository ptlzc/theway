# Textarea modification rules

This file contains the complete crate-local modification rules for `theway-ratatui-textarea`. Read the state model in [`docs/architecture.md`](docs/architecture.md) before changing editor behavior.

## Ownership

- Keep application commands, session state, daemon protocol, and theme selection outside this crate.
- Preserve grapheme-boundary normalization for all external cursor and edit ranges.
- Treat atomic element ranges as indivisible in movement, selection, deletion, wrapping, and mouse hit testing.
- Preserve buffer identity and generation checks when extending `EditPlan`.
- Use terminal display width for visual columns and keep logical-to-visual mappings style-preserving.

## Interaction

- Route key interpretation through [`classify_key_event`](src/editor_keys.rs) so editor and widget behavior do not diverge.
- Keep undo grouping aligned with a user-visible editing action and clear redo history only when a new branch is committed.
- Preserve the platform-specific AltGr distinction in [`src/lib.rs`](src/lib.rs).
- Report atomic-element interactions through `TextElementEvent`; the embedding application decides their meaning.

## Compatibility

- Keep code-lineage details in [`NOTICE`](NOTICE).
- Update the demo when a public interaction contract changes.
- Place multi-file suites under `tests/<mirrored-src-path>/` and bridge them from the owning source module.

## Verification

This crate is excluded from the root workspace and declares its own `[workspace]`; run these checks from the repository root with `--manifest-path`.

Run `cargo test --manifest-path crates/theway-ratatui-textarea/Cargo.toml`, `cargo check --manifest-path crates/theway-ratatui-textarea/Cargo.toml --example textarea_demo`, and `cargo doc --manifest-path crates/theway-ratatui-textarea/Cargo.toml --no-deps --document-private-items`.
