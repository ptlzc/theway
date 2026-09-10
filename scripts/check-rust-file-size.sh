#!/usr/bin/env bash
set -euo pipefail

readonly max_lines=800

# Vendored/ported crates (root Cargo.toml `[workspace.exclude]`) keep upstream file shapes,
# so size governance covers first-party crates only. `crates/mermaid-parser` and
# `crates/tgrep-*` miss the `crates/theway-*` prefix below; the four vendored `theway-*`
# ports are skipped explicitly.
failed=0
while IFS= read -r -d '' path; do
  lines=$(wc -l < "$path")
  if (( lines > max_lines )); then
    printf '%s: %d lines (limit: %d)\n' "$path" "$lines" "$max_lines" >&2
    failed=1
  fi
done < <(find crates -type f -name '*.rs' -path 'crates/theway-*/*' \
  -not -path 'crates/theway-markdown/*' \
  -not -path 'crates/theway-markdown-core/*' \
  -not -path 'crates/theway-pager-render/*' \
  -not -path 'crates/theway-ratatui-textarea/*' \
  -print0)

if (( failed != 0 )); then
  printf 'split the files above into domain modules\n' >&2
  exit 1
fi

printf 'all theway-* Rust files are at most %d lines\n' "$max_lines"
