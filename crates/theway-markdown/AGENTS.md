# Markdown renderer modification rules

This file contains the complete crate-local modification rules for `theway-markdown`. Read the pipeline contract in [`docs/architecture.md`](docs/architecture.md) before changing renderer behavior.

## Ownership

- Keep application feed state, input events, and transport concerns outside this crate.
- Change shared parser options or strikethrough interpretation in `theway-markdown-core`, then verify both crates.
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
- Add focused multi-file suites under `tests/<mirrored-src-path>/` and bridge them from the owning source module.

## Verification

Run `cargo test -p theway-markdown-core -p theway-markdown` and `cargo doc -p theway-markdown --no-deps --document-private-items`.
