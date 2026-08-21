#!/usr/bin/env bash
# =============================================================================
# sdk-publish — 手动发布客户端 TS SDK 到 npm 官方公共仓库。
#
# 用法:
#   scripts/sdk-publish.sh [patch|minor|major] ["#<issue>"]
#
# 流程:
#   1. crates/theway-transport/proto/ 与 sdks/client/ 必须无未提交改动
#   2. 重新生成并校验与 HEAD 一致 (仓库无漂移; 漂移说明 proto 改了没 commit)
#   3. 验证 npm 官方仓库登录身份
#   4. npm version <bump> (不打 tag 不自动 commit, 由本脚本统一 commit)
#   5. npm publish --access public 到 npm 官方仓库
#   6. commit 版本号 + push origin main (gitlab 镜像请手动补推)
#
# 鉴权:
#   npm login --registry=https://registry.npmjs.org/
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PKG="@theway-ai/sdk"
REGISTRY="https://registry.npmjs.org/"
EXPECTED_NPM_USER="theway-ai"
BUMP="${1:-minor}"
ISSUE="${2:-}"

case "$BUMP" in
  patch|minor|major) ;;
  *)
    echo "usage: scripts/sdk-publish.sh [patch|minor|major] [\"#<issue>\"]" >&2
    exit 2
    ;;
esac

# 1. crates/theway-transport/proto/ 与 sdks/client/ 必须干净
if [ -n "$(git status --porcelain -- crates/theway-transport/proto/ sdks/client/)" ]; then
  echo "error: crates/theway-transport/proto/ 或 sdks/client/ 有未提交改动, 先 commit/stash" >&2
  git status --porcelain -- crates/theway-transport/proto/ sdks/client/ >&2
  exit 1
fi

# 2. 重新生成, 若与 HEAD 有 diff 说明仓库漂移 (proto 已改但生成产物未提交)
bash scripts/sdk-sync.sh
if ! git diff --quiet -- sdks/client/; then
  echo "error: 重新生成后 sdks/client/ 与 HEAD 不一致 — proto 改动尚未同步提交" >&2
  git diff --stat -- sdks/client/ >&2
  exit 1
fi

# 3. 验证 npm 官方仓库登录身份
if ! NPM_USER="$(npm whoami --registry "$REGISTRY")"; then
  echo "error: npm 官方仓库未登录; 请先运行 npm login --registry=$REGISTRY" >&2
  exit 1
fi
if [ "$NPM_USER" != "$EXPECTED_NPM_USER" ]; then
  echo "error: npm 当前用户为 $NPM_USER, 发布 $PKG 需要 $EXPECTED_NPM_USER" >&2
  exit 1
fi
echo "[sdk-publish] npm user: $NPM_USER"

# 4. bump 版本 (package.json + package-lock.json, 不产生 git tag/commit)
CUR="$(node -p "require('./sdks/client/package.json').version")"
(cd sdks/client && npm version "$BUMP" --no-git-tag-version >/dev/null)
NEW="$(node -p "require('./sdks/client/package.json').version")"

# 5. 新版本不得已存在于 registry
if npm view "$PKG@$NEW" version --registry "$REGISTRY" >/dev/null 2>&1; then
  echo "error: $PKG@$NEW 已在 registry 发布过, 请选更大的 bump" >&2
  exit 1
fi

# 6. 发布到 npm 官方公共仓库
echo "[sdk-publish] publish $PKG $CUR -> $NEW"
(cd sdks/client && npm publish --registry "$REGISTRY" --access public)

# 7. commit 版本号并推送
git add sdks/client/package.json sdks/client/package-lock.json
git commit -m "chore(sdk): release $PKG v$NEW $ISSUE"
git push origin main

echo
echo "[sdk-publish] done: $PKG@$NEW"
echo "  下游更新: npm install $PKG@$NEW"
echo "  gitlab 镜像若需同步: git push gitlab main"
