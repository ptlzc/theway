#!/usr/bin/env bash
set -euo pipefail

readonly max_lines=800

failed=0
while IFS= read -r -d '' path; do
  lines=$(wc -l < "$path")
  if (( lines > max_lines )); then
    printf '%s: %d lines (limit: %d)\n' "$path" "$lines" "$max_lines" >&2
    failed=1
  fi
done < <(find crates -type f -name '*.rs' -path 'crates/theway-*/*' -print0)

if (( failed != 0 )); then
  printf 'split the files above into domain modules\n' >&2
  exit 1
fi

printf 'all theway-* Rust files are at most %d lines\n' "$max_lines"
