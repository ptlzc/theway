#!/usr/bin/env bash
# Regenerate src/generated/ from proto/*.proto (ts-proto, grpc-js client-only).
# Requires devDependencies installed (grpc-tools bundles protoc; ts-proto provides the plugin).
set -euo pipefail
cd "$(dirname "$0")/.."

PROTOC="node_modules/.bin/grpc_tools_node_protoc"
PLUGIN="node_modules/.bin/protoc-gen-ts_proto"
OUT="src/generated"

if [ ! -x "$PROTOC" ] || [ ! -x "$PLUGIN" ]; then
  echo "error: run 'npm install' first (grpc-tools + ts-proto required)" >&2
  exit 1
fi

mkdir -p "$OUT"
rm -f "$OUT"/*.ts

# Each domain proto file carries its own service (plus health.proto);
# protoc resolves the domain imports and ts-proto writes one generated file
# per proto file.
#
# ts-proto options:
#   outputClientImpl=grpc-js  — client stubs only (no server side)
#   esModuleInterop=true      — ES module imports
#   useOptionals=true         — proto3 fields optional (no fake ""/0 defaults)
#   forceLong=string          — int64/uint64 as string (matches proto-loader longs: String)
#   oneof=unions              — oneof as discriminated $case unions
#   importSuffix=.js          — NodeNext-compatible relative imports between split files
"$PROTOC" \
  --plugin="protoc-gen-ts_proto=$PLUGIN" \
  --ts_proto_out="$OUT" \
  --proto_path=proto \
  --ts_proto_opt=outputServices=grpc-js,outputClientImpl=grpc-js,esModuleInterop=true,useOptionals=messages,forceLong=string,oneof=unions,importSuffix=.js \
  proto/commands.proto proto/session.proto proto/graph_engine.proto proto/events.proto proto/settings.proto proto/health.proto

echo "generated:"
ls "$OUT"
