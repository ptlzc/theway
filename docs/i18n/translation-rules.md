# Translation rules

English | [中文](translation-rules.zh.md)

These rules apply when creating or updating the Chinese counterpart of an English-first crate document. English defines the default source; the translation preserves the same behavior, qualifications, and examples while reading as natural technical Chinese.

## Fidelity

- Translate every source clause without adding behavior, prerequisites, warnings, versions, examples, or implementation status.
- Describe the current mechanism rather than the change history in both languages.
- If the pair disagrees, correct the English source when necessary, then synchronize the Chinese counterpart in the same change.

## Structure preservation

- Preserve heading depth and order, list type and item count, table rows and columns, and emphasis spans.
- Keep fenced code blocks byte-identical, including their information strings, comments, and line endings.
- Keep inline code, commands, flags, configuration keys, paths, API names, event names, versions, and numeric values verbatim.
- Keep every ordinary link target identical across the pair and point both sides at the unsuffixed English document. Within crate documentation, repository-relative links stay inside the owning crate; describe root or sibling-crate concepts without links. Only the language switcher uses a `.zh.md` target.

## Terminology

- Follow [terminology.md](terminology.md) for terms listed there.
- Preserve an unlisted technical term in English when no established Chinese rendering is unambiguous.
- Preserve canonical product and protocol casing, including GitHub, Rust, TypeScript, JSON-RPC, gRPC, MCP, SSE, and WebSocket.

## Chinese typography

- Put one half-width space between Chinese text and Latin words or numerals.
- Use full-width Chinese punctuation in Chinese prose; keep punctuation inside code spans unchanged.
- Use direct, professional technical Chinese with explicit actors and one physical line per paragraph.

## Review and recording

- Review meaning and language before recording hashes; the verifier cannot judge translation quality.
- Run `scripts/verify-doc-i18n.py --write <source.md>` only after both sides are synchronized.
- Run `make doc-sync` before committing any paired-document change.
