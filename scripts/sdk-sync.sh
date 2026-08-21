#!/usr/bin/env bash
# =============================================================================
# sdk-sync — 把 crates/theway-transport/proto/ (唯一事实源) 同步到 sdks/client/，并重新生成 TS 客户端。
#
#   1. cp crates/theway-transport/proto/*.proto -> sdks/client/proto/
#   2. npm run gen                      (ts-proto -> sdks/client/src/generated/*.ts)
#
# 幂等: proto 未变时重复执行不产生任何 diff (ts-proto 版本由 package-lock 固定)。
# 依赖 node_modules (缺失时自动 npm install)。
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[sdk-sync] sync crates/theway-transport/proto -> sdks/client/proto"
cp crates/theway-transport/proto/*.proto sdks/client/proto/

cd "$ROOT/sdks/client"
if [ ! -d node_modules ]; then
  echo "[sdk-sync] node_modules missing, npm install ..."
  npm install --no-audit --no-fund --loglevel=error
fi

echo "[sdk-sync] regenerate src/generated"
npm run gen >/dev/null

echo "[sdk-sync] done"
