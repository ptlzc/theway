# Bilingual crate documentation

English | [中文](README.zh.md)

The crate documentation corpus uses English as its default source and Simplified Chinese as a reviewed auxiliary translation. This document owns the file-pairing contract; [translation-rules.md](translation-rules.md) owns translation fidelity and structure, and [terminology.md](terminology.md) owns shared terminology.

## Pairing contract

- An authored pair consists of three sibling files: English `foo.md`, Chinese `foo.zh.md`, and `foo.i18n.yaml`.
- The unsuffixed English file is the default entry point and source of truth. The Chinese file must express the same current mechanism without adding or dropping behavior.
- Both Markdown files carry a language switcher immediately after the H1 heading. Ordinary links in both files target unsuffixed English paths; only the switcher links to `.zh.md`.
- The sidecar records each file's Git blob hash at the last reviewed synchronization. Editing either side without updating the other side and re-recording the pair fails `make doc-sync`.
- Heading levels, list shape, table shape, link targets, and fenced code blocks remain structurally aligned. Code blocks are byte-identical.

## Scope

The contract covers every workspace member's `README.md`, required `docs/architecture.md`, additional Markdown under each crate's `docs/` directory, and the paired policy documents in this directory.

- Root and crate `AGENTS.md` files are English-only agent instructions.
- Root product and contributor documentation outside this policy remains outside the crate-pairing corpus.

## Update workflow

1. Edit the English source and make the smallest corresponding update to the Chinese file in the same change.
2. Preserve code spans, code blocks, commands, paths, API names, link targets, list shape, and table shape according to [translation-rules.md](translation-rules.md).
3. Confirm both files say the same thing, then run `scripts/verify-doc-i18n.py --write <source.md>`.
4. Run `make doc-sync`; never hand-edit the hashes in `*.i18n.yaml`.

## Verification

```bash
scripts/verify-doc-i18n.py --list
scripts/verify-doc-i18n.py --write crates/theway-core/README.md
make doc-sync
```

`scripts/verify-doc-i18n.py` checks completeness, language switchers, recorded hashes, and Markdown structure. A successful check proves that the reviewed file contents were recorded together; human review remains responsible for semantic and linguistic quality.
