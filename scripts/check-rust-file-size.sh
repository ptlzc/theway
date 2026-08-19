#!/usr/bin/env bash
set -euo pipefail

readonly max_lines=800
readonly exemptions=(
  "crates/mermaid-parser/src/parser.rs"
  "crates/theway-core/src/agent/assembly.rs"
)

is_exempt() {
  local path=$1
  local exemption
  for exemption in "${exemptions[@]}"; do
    if [[ "$path" == "$exemption" ]]; then
      return 0
    fi
  done
  return 1
}

failed=0
while IFS= read -r -d '' path; do
  if is_exempt "$path"; then
    continue
  fi
  lines=$(wc -l < "$path")
  if (( lines > max_lines )); then
    printf '%s: %d lines (limit: %d)\n' "$path" "$lines" "$max_lines" >&2
    failed=1
  fi
done < <(find crates -type f -name '*.rs' -path 'crates/theway-*/*' -print0)

if (( failed != 0 )); then
  printf 'split the files above by domain or document an exemption in AGENTS.md\n' >&2
  exit 1
fi

printf 'all non-exempt theway-* Rust files are at most %d lines\n' "$max_lines"
