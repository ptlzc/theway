# Textarea modification rules

This file applies to `crates/theway-ratatui-textarea/`. Follow the workspace rules in [`../../AGENTS.md`](../../AGENTS.md) and the state model in [`docs/architecture.md`](docs/architecture.md).

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
- Follow the mirrored test layout in [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) for multi-file suites.

## Verification

Run `cargo test -p theway-ratatui-textarea`, `cargo check -p theway-ratatui-textarea --example textarea_demo`, and `cargo doc -p theway-ratatui-textarea --no-deps --document-private-items`.
