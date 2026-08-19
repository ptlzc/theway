# Markdown core modification rules

This file contains the complete crate-local modification rules for `theway-markdown-core`. Read the mechanism in [`docs/architecture.md`](docs/architecture.md) before changing parser behavior.

## Ownership

- Keep this crate headless. Do not add ratatui, syntect, terminal capability, UI state, or transport dependencies.
- Keep [`parser_options`](src/lib.rs) as the single parser-feature definition and route consumers through [`offset_events`](src/lib.rs).
- Preserve source ranges as UTF-8 byte offsets into the caller's original input.
- Add a [`StructuralIssue`](src/lib.rs) only for a bounded render-fidelity check; normal CommonMark fallback is not an error.

## Compatibility

- Preserve the double-tilde-only strikethrough policy unless the renderer and analysis contract change together.
- Update [`MarkdownStats::as_pairs`](src/lib.rs) whenever a statistics field changes; its exhaustive mapping is part of the drift check.
- Keep code-lineage details in [`NOTICE`](NOTICE), not in API or architecture prose.

## Verification

Run `cargo test -p theway-markdown-core` and `cargo doc -p theway-markdown-core --no-deps --document-private-items`. Changes to parser events also require `cargo test -p theway-markdown`.
