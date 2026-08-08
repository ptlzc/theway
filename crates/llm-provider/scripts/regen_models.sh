#!/usr/bin/env bash
# Regenerate src/models.generated.json from the TS source of truth.
# Run when the TS catalog at packages/ai/src/models.generated.ts changes.
set -euo pipefail
HERE="$(cd "$(dirname "$0")"/.. && pwd)"
TS_PATH="${TS_PATH:-/Users/dongxu/pi-rs/packages/ai/src/models.generated.ts}"

if [[ ! -f "$TS_PATH" ]]; then
    echo "TS catalog not found at $TS_PATH — set TS_PATH=/abs/path/to/models.generated.ts" >&2
    exit 1
fi

node --experimental-strip-types --no-warnings -e "
const { MODELS } = await import('$TS_PATH');
process.stdout.write(JSON.stringify(MODELS));
" > "$HERE/src/models.generated.json"

echo "regenerated $HERE/src/models.generated.json ($(wc -c < "$HERE/src/models.generated.json") bytes)"
