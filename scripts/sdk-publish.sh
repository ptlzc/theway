#!/usr/bin/env bash
# =============================================================================
# sdk-publish — 手动发布 TS SDK 到内网 Nexus (npm-private)。
#
# 用法:
#   scripts/sdk-publish.sh [patch|minor|major] ["#<issue>"]
#
# 流程:
#   1. crates/theway-transport/proto/ 与 sdk/ 必须无未提交改动
#   2. 重新生成并校验与 HEAD 一致 (仓库无漂移; 漂移说明 proto 改了没 commit)
#   3. npm version <bump> (不打 tag 不自动 commit, 由本脚本统一 commit)
#   4. npm publish (registry 取 sdk/package.json 的 publishConfig = 内网 Nexus)
#   5. commit 版本号 + push origin main (gitlab 镜像请手动补推)
#
# 鉴权: 发布机 ~/.npmrc 需含 //registry.npmjs.org/... 的 _authToken。
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PKG="@theway-ai/sdk"
BUMP="${1:-minor}"
ISSUE="${2:-}"

case "$BUMP" in
  patch|minor|major) ;;
  *)
    echo "usage: scripts/sdk-publish.sh [patch|minor|major] [\"#<issue>\"]" >&2
    exit 2
    ;;
esac

# 1. crates/theway-transport/proto/ 与 sdk/ 必须干净
if [ -n "$(git status --porcelain -- crates/theway-transport/proto/ sdk/)" ]; then
  echo "error: crates/theway-transport/proto/ 或 sdk/ 有未提交改动, 先 commit/stash" >&2
  git status --porcelain -- crates/theway-transport/proto/ sdk/ >&2
  exit 1
fi

# 2. 重新生成, 若与 HEAD 有 diff 说明仓库漂移 (proto 已改但生成产物未提交)
bash scripts/sdk-sync.sh
if ! git diff --quiet -- sdk/; then
  echo "error: 重新生成后 sdk/ 与 HEAD 不一致 — proto 改动尚未同步提交" >&2
  git diff --stat -- sdk/ >&2
  exit 1
fi

# 3. bump 版本 (package.json + package-lock.json, 不产生 git tag/commit)
CUR="$(node -p "require('./sdk/package.json').version")"
(cd sdk && npm version "$BUMP" --no-git-tag-version >/dev/null)
NEW="$(node -p "require('./sdk/package.json').version")"

# 4. 新版本不得已存在于 registry
if npm view "$PKG@$NEW" version >/dev/null 2>&1; then
  echo "error: $PKG@$NEW 已在 registry 发布过, 请选更大的 bump" >&2
  exit 1
fi

# 5. 发布到内网 Nexus
echo "[sdk-publish] publish $PKG $CUR -> $NEW"
(cd sdk && npm publish)

# 6. commit 版本号并推送
git add sdk/package.json sdk/package-lock.json
git commit -m "chore(sdk): release $PKG v$NEW $ISSUE"
git push origin main

echo
echo "[sdk-publish] done: $PKG@$NEW"
echo "  下游更新: npm install $PKG@$NEW"
echo "  gitlab 镜像若需同步: git push gitlab main"
