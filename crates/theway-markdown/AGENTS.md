# Markdown renderer modification rules

This file applies to `crates/theway-markdown/`. Follow the workspace rules in [`../../AGENTS.md`](../../AGENTS.md) and the pipeline contract in [`docs/architecture.md`](docs/architecture.md).

## Ownership

- Keep application feed state, input events, and transport concerns outside this crate.
- Change shared parser options or strikethrough interpretation in [`theway-markdown-core`](../theway-markdown-core/AGENTS.md), then verify both crates.
- Preserve source maps, hyperlink ranges, and code-block spans when adding a render transform.
- Use display width and grapheme boundaries for terminal layout.

## Streaming contract

- A completed streaming render must agree with the one-shot render for the same normalized source and settings.
- Freeze a checkpoint only when later chunks cannot alter its parsed or rendered meaning.
- Thread link identifiers and open-code highlighting state through tail re-renders; do not derive stable metadata from rendered text.
- Keep Mermaid parsing and layout bounded, with a readable source fallback.

## Compatibility

- Keep code-lineage details in [`NOTICE`](NOTICE).
- Preserve the intent of local changes when updating code with shared lineage, including the parser policy, terminal color adaptation, and width limits.
- Add focused tests under the mirrored test layout described by [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md).

## Verification

Run `cargo test -p theway-markdown-core -p theway-markdown` and `cargo doc -p theway-markdown --no-deps --document-private-items`.
